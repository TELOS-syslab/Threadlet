// SPDX-License-Identifier: MPL-2.0

//! This module defines the socket table, which manages all TCP and UDP sockets,
//! for efficiently inserting, looking up, and removing sockets.

use alloc::{boxed::Box, sync::Arc, vec::Vec};
use core::{
    net::Ipv4Addr,
    sync::atomic::{AtomicUsize, Ordering},
};

use jhash::{jhash_1vals, jhash_3vals};
use smoltcp::wire::{IpAddress, IpEndpoint, IpListenEndpoint};
use static_assertions::const_assert;
use ostd::sync::{LocalIrqDisabled, SpinLock};

use crate::{
    ext::Ext,
    socket::{TcpConnectionBg, TcpListenerBg, UdpSocketBg},
    wire::PortNum,
};

pub type SocketHash = u32;

/// A unique key for identifying a `TcpListener`.
///
/// Note that two `TcpListener`s cannot listen on the same address
/// even if both sockets set SO_REUSEADDR to true,
/// so there cannot be multiple listeners with the same `ListenerKey`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ListenerKey {
    addr: IpAddress,
    port: PortNum,
    hash: SocketHash,
}

impl ListenerKey {
    pub(crate) const fn new(addr: IpAddress, port: PortNum) -> Self {
        // FIXME: If the socket is listening on an unspecified address (0.0.0.0),
        // Linux will get the hash value by port only.
        let hash = hash_addr_port(addr, port);
        Self { addr, port, hash }
    }

    pub(crate) const fn hash(&self) -> SocketHash {
        self.hash
    }
}

impl From<IpListenEndpoint> for ListenerKey {
    fn from(listen_endpoint: IpListenEndpoint) -> Self {
        let addr = listen_endpoint
            .addr
            .unwrap_or(IpAddress::Ipv4(Ipv4Addr::UNSPECIFIED));
        let port = listen_endpoint.port;
        Self::new(addr, port)
    }
}

/// A unique key for identifying a `TcpConnection`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ConnectionKey {
    local_addr: IpAddress,
    local_port: PortNum,
    remote_addr: IpAddress,
    remote_port: PortNum,
    hash: SocketHash,
}

impl ConnectionKey {
    pub(crate) const fn new(
        local_addr: IpAddress,
        local_port: PortNum,
        remote_addr: IpAddress,
        remote_port: PortNum,
    ) -> Self {
        let hash = hash_local_remote(local_addr, local_port, remote_addr, remote_port);
        Self {
            local_addr,
            local_port,
            remote_addr,
            remote_port,
            hash,
        }
    }

    pub(crate) const fn hash(&self) -> SocketHash {
        self.hash
    }
}

impl From<(IpEndpoint, IpEndpoint)> for ConnectionKey {
    fn from(value: (IpEndpoint, IpEndpoint)) -> Self {
        Self::new(value.0.addr, value.0.port, value.1.addr, value.1.port)
    }
}

// FIXME: The following two constants should be randomly-generated at runtime
const HASH_SECRET: u32 = 0xdeadbeef;

// FIXME: This constant should be a per-net-namespace value
const NET_HASHMIX: u32 = 0xbeefdead;

const fn hash_local_remote(
    local_addr: IpAddress,
    local_port: PortNum,
    remote_addr: IpAddress,
    remote_port: PortNum,
) -> SocketHash {
    // FIXME: Deal with IPv6 addresses once IPv6 is supported.
    let IpAddress::Ipv4(local_ipv4) = local_addr;
    let IpAddress::Ipv4(remote_ipv4) = remote_addr;

    jhash_3vals(
        local_ipv4.to_bits(),
        remote_ipv4.to_bits(),
        (local_port as u32).wrapping_shl(16) | remote_port as u32,
        HASH_SECRET.wrapping_add(NET_HASHMIX),
    )
}

const fn hash_addr_port(addr: IpAddress, port: PortNum) -> SocketHash {
    // FIXME: Deal with IPv6 addresses once IPv6 is supported.
    let IpAddress::Ipv4(ipv4_addr) = addr;

    jhash_1vals(ipv4_addr.to_bits(), NET_HASHMIX) ^ (port as u32)
}

/// The socket table manages TCP and UDP sockets.
///
/// Unlike the Linux inet hashtable, which is shared across a single network namespace,
/// this table is currently limited to a single interface.
///

// to support INADDR_ANY (0.0.0.0).
pub(crate) struct SocketTable<E: Ext> {

    // the first is hashed by local address and port,
    // the second is hashed by local port only.
    // The second table is the only place where sockets listening on INADDR_ANY (0.0.0.0) can exist.
    // Since we do not yet support INADDR_ANY, we only have the first table here.
    listener_buckets: Box<[SpinLock<ListenerHashBucket<E>, LocalIrqDisabled>]>,
    connection_buckets: Box<[SpinLock<ConnectionHashBucket<E>, LocalIrqDisabled>]>,
    // Linux does not include UDP sockets in the inet hashtable.
    // Here we include UDP sockets in the socket table for simplicity.
    // Note that multiple UDP sockets can be bound to the same address,
    // so we cannot use (addr, port) as a _unique_ key for UDP sockets.
    // We keep a single global list for UDP sockets, which is used both for
    // lookup helpers and snapshotting.
    udp_sockets: SpinLock<Vec<Arc<UdpSocketBg<E>>>, LocalIrqDisabled>,
    /// Global list of all TCP listeners for fast snapshotting.
    listener_all: SpinLock<Vec<Arc<TcpListenerBg<E>>>, LocalIrqDisabled>,
    /// Global list of all TCP connections for fast snapshotting.
    connection_all: SpinLock<Vec<Arc<TcpConnectionBg<E>>>, LocalIrqDisabled>,
    /// Total number of TCP connections in this table.
    tcp_conn_count: AtomicUsize,
    /// Number of pending dead TCP connections to clean up.
    dead_conn_pending: AtomicUsize,
    /// Queue of dead TCP connections awaiting cleanup.
    dead_conns: SpinLock<Vec<Arc<TcpConnectionBg<E>>>, LocalIrqDisabled>,
}

// On Linux, the number of buckets is determined at runtime based on the available memory.
// For Asterinas we pick a fixed, conservative value suitable for a 4GiB system.
// Each bucket holds a small Vec and SpinLock; 4096 buckets keeps memory overhead
// in the hundreds of KiB while greatly reducing contention.
// The bucket count must be a power of 2 to ensure efficient modulo calculations.
const LISTENER_BUCKET_COUNT: u32 = 1 << 10; // 1024 buckets
const LISTENER_BUCKET_MASK: u32 = LISTENER_BUCKET_COUNT - 1;
const CONNECTION_BUCKET_COUNT: u32 = 1 << 10; // 1024 buckets
const CONNECTION_BUCKET_MASK: u32 = CONNECTION_BUCKET_COUNT - 1;

const_assert!(LISTENER_BUCKET_COUNT.is_power_of_two());
const_assert!(CONNECTION_BUCKET_COUNT.is_power_of_two());

impl<E: Ext> SocketTable<E> {
    pub(crate) fn new() -> Self {
        let listener_buckets = (0..LISTENER_BUCKET_COUNT)
            .map(|_| SpinLock::new(ListenerHashBucket::new()))
            .collect();

        let connection_buckets = (0..CONNECTION_BUCKET_COUNT)
            .map(|_| SpinLock::new(ConnectionHashBucket::new()))
            .collect();

        let udp_sockets = SpinLock::new(Vec::new());
        let listener_all = SpinLock::new(Vec::new());
        let connection_all = SpinLock::new(Vec::new());
        let dead_conns = SpinLock::new(Vec::new());

        Self {
            listener_buckets,
            connection_buckets,
            udp_sockets,
            listener_all,
            connection_all,
            tcp_conn_count: AtomicUsize::new(0),
            dead_conn_pending: AtomicUsize::new(0),
            dead_conns,
        }
    }

    /// Inserts a TCP listener into the table.
    ///
    /// If a socket with the same [`ListenerKey`] has already been inserted,
    /// this method will return an error and the listener will not be inserted.
    pub(crate) fn insert_listener(
        &self,
        listener: Arc<TcpListenerBg<E>>,
    ) -> Result<(), Arc<TcpListenerBg<E>>> {
        let key = listener.listener_key();

        let bucket = {
            let hash = key.hash();
            let bucket_index = hash & LISTENER_BUCKET_MASK;
            &self.listener_buckets[bucket_index as usize]
        };

        {
            let mut bucket = bucket.lock();
            if bucket
                .listeners
                .iter()
                .any(|tcp_listener| tcp_listener.listener_key() == listener.listener_key())
            {
                return Err(listener);
            }

            bucket.listeners.push(listener.clone());
        }

        {
            let mut all = self.listener_all.lock();
            debug_assert!(!all.iter().any(|l| Arc::ptr_eq(l, &listener)));
            all.push(listener.clone());
        }

        Ok(())
    }

    pub(crate) fn insert_connection(
        &self,
        connection: Arc<TcpConnectionBg<E>>,
    ) -> Result<(), Arc<TcpConnectionBg<E>>> {
        let key = connection.connection_key();

        let bucket = {
            let hash = key.hash();
            let bucket_index = hash & CONNECTION_BUCKET_MASK;
            &self.connection_buckets[bucket_index as usize]
        };

        {
            let mut bucket = bucket.lock();
            if bucket
                .connections
                .iter()
                .any(|tcp_connection| tcp_connection.connection_key() == connection.connection_key())
            {
                return Err(connection);
            }

            bucket.connections.push(connection.clone());
        }

        {
            let mut all = self.connection_all.lock();
            debug_assert!(!all.iter().any(|c| Arc::ptr_eq(c, &connection)));
            all.push(connection.clone());
        }
        self.tcp_conn_count.fetch_add(1, Ordering::Release);

        Ok(())
    }

    pub(crate) fn insert_udp_socket(&self, udp_socket: Arc<UdpSocketBg<E>>) {
        let mut list = self.udp_sockets.lock();
        debug_assert!(!list.iter().any(|socket| Arc::ptr_eq(socket, &udp_socket)));
        list.push(udp_socket);
    }

    pub(crate) fn lookup_listener_arc(&self, key: &ListenerKey) -> Option<Arc<TcpListenerBg<E>>> {
        let bucket = {
            let hash = key.hash();
            let bucket_index = hash & LISTENER_BUCKET_MASK;
            &self.listener_buckets[bucket_index as usize]
        };

        let bucket = bucket.lock();
        bucket
            .listeners
            .iter()
            .find(|listener| listener.listener_key() == key)
            .cloned()
    }

    pub(crate) fn lookup_connection_arc(
        &self,
        key: &ConnectionKey,
    ) -> Option<Arc<TcpConnectionBg<E>>> {
        let bucket = {
            let hash = key.hash();
            let bucket_index = hash & CONNECTION_BUCKET_MASK;
            &self.connection_buckets[bucket_index as usize]
        };

        let bucket = bucket.lock();
        bucket
            .connections
            .iter()
            .find(|connection| connection.connection_key() == key)
            .cloned()
    }

    pub(crate) fn remove_listener(
        &self,
        listener: &TcpListenerBg<E>,
    ) -> Option<Arc<TcpListenerBg<E>>> {
        let key = listener.listener_key();

        let bucket = {
            let hash = key.hash();
            let bucket_index = hash & LISTENER_BUCKET_MASK;
            &self.listener_buckets[bucket_index as usize]
        };

        let removed = {
            let mut bucket = bucket.lock();
            let index = bucket
                .listeners
                .iter()
                .position(|tcp_listener| tcp_listener.listener_key() == listener.listener_key())?;
            Some(bucket.listeners.swap_remove(index))
        };

        if let Some(ref removed_arc) = removed {
            let mut all = self.listener_all.lock();
            if let Some(index) = all
                .iter()
                .position(|tcp_listener| Arc::ptr_eq(tcp_listener, removed_arc))
            {
                all.swap_remove(index);
            }
        }

        removed
    }

    pub(crate) fn remove_udp_socket(
        &self,
        socket: &Arc<UdpSocketBg<E>>,
    ) -> Option<Arc<UdpSocketBg<E>>> {
        let mut list = self.udp_sockets.lock();
        let index = list
            .iter()
            .position(|udp_socket| Arc::ptr_eq(udp_socket, socket))?;
        Some(list.swap_remove(index))
    }

    pub(crate) fn remove_dead_tcp_connections(&self) {
        if self.dead_conn_pending.load(Ordering::Acquire) == 0 {
            return;
        }

        let dead_conns = {
            let mut queue = self.dead_conns.lock();
            if queue.is_empty() {
                return;
            }
            let dead = core::mem::take(&mut *queue);
            self.dead_conn_pending
                .fetch_sub(dead.len(), Ordering::AcqRel);
            dead
        };

        for conn in dead_conns {
            let key = conn.connection_key();
            let bucket_index = key.hash() & CONNECTION_BUCKET_MASK;
            let removed = {
                let mut bucket = self.connection_buckets[bucket_index as usize].lock();
                if let Some(index) = bucket.connections.iter().position(|c| Arc::ptr_eq(c, &conn)) {
                    bucket.connections.swap_remove(index);
                    true
                } else {
                    false
                }
            };

            if removed {
                conn.clone().on_dead_events();
                let mut all = self.connection_all.lock();
                if let Some(index) = all.iter().position(|c| Arc::ptr_eq(c, &conn)) {
                    all.swap_remove(index);
                    self.tcp_conn_count.fetch_sub(1, Ordering::Release);
                }
            }
        }
    }

    pub(crate) fn mark_dead_tcp_connection(&self, conn: &Arc<TcpConnectionBg<E>>) {
        let mut queue = self.dead_conns.lock();
        queue.push(conn.clone());
        self.dead_conn_pending.fetch_add(1, Ordering::Release);
    }

    pub(crate) fn tcp_listeners_snapshot(&self) -> Vec<Arc<TcpListenerBg<E>>> {
        self.listener_all.lock().clone()
    }

    pub(crate) fn tcp_connections_snapshot(&self) -> Vec<Arc<TcpConnectionBg<E>>> {
        self.connection_all.lock().clone()
    }

    pub(crate) fn udp_sockets_snapshot(&self) -> Vec<Arc<UdpSocketBg<E>>> {
        self.udp_sockets.lock().clone()
    }
}

impl<E: Ext> Default for SocketTable<E> {
    fn default() -> Self {
        Self::new()
    }
}

struct ListenerHashBucket<E: Ext> {
    listeners: Vec<Arc<TcpListenerBg<E>>>,
}

impl<E: Ext> ListenerHashBucket<E> {
    const fn new() -> Self {
        Self {
            listeners: Vec::new(),
        }
    }
}

struct ConnectionHashBucket<E: Ext> {
    connections: Vec<Arc<TcpConnectionBg<E>>>,
}

impl<E: Ext> ConnectionHashBucket<E> {
    const fn new() -> Self {
        Self {
            connections: Vec::new(),
        }
    }
}
