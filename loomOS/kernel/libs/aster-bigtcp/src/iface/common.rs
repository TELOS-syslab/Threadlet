// SPDX-License-Identifier: MPL-2.0

use alloc::{
    collections::btree_map::{BTreeMap, Entry},
    string::String,
    sync::Arc,
    vec::Vec,
};

use ostd::sync::{LocalIrqDisabled, SpinLock, SpinLockGuard};
use smoltcp::{
    iface::{packet::Packet, Context},
    phy::{ChecksumCapabilities, Device},
    wire::{IpAddress, IpEndpoint, Ipv4Address, Ipv4Packet},
};

use super::{
    poll::{FnHelper, PollContext},
    port::BindPortConfig,
    time::get_network_timestamp,
    Iface,
};
use crate::{
    errors::BindError,
    ext::Ext,
    socket::{TcpListenerBg, UdpSocketBg},
    socket_table::SocketTable,
};

/// Provides short-lived access to smoltcp Context behind the interface lock.
/// This is used to avoid holding the interface lock for the entire poll path.
pub(crate) struct CxAccess<'a, E: Ext> {
    pub(super) common: &'a IfaceCommon<E>,
}

impl<'a, E: Ext> CxAccess<'a, E> {
    #[inline]
    pub(super) fn with<R>(&self, f: impl FnOnce(&mut Context) -> R) -> R {
        let mut interface = self.common.interface.lock();
        let cx = interface.context();
        // Refresh timestamp for this short-lived context access.
        cx.now = super::time::get_network_timestamp();
        f(cx)
    }

    #[inline]
    pub(super) fn ipv4_addr(&self) -> Option<Ipv4Address> {
        self.common.interface.lock().ipv4_addr()
    }

    #[inline]
    pub(super) fn in_same_network(&self, addr: &IpAddress) -> bool {
        self.with(|cx| cx.in_same_network(addr))
    }
}

pub struct IfaceCommon<E: Ext> {
    name: String,
    interface: SpinLock<smoltcp::iface::Interface, LocalIrqDisabled>,
    used_ports: SpinLock<BTreeMap<u16, usize>, LocalIrqDisabled>,
    // Socket table now manages its own fine-grained locks; no outer lock here.
    sockets: SocketTable<E>,
    sched_poll: E::ScheduleNextPoll,
}

impl<E: Ext> IfaceCommon<E> {
    pub(super) fn new(
        name: String,
        interface: smoltcp::iface::Interface,
        sched_poll: E::ScheduleNextPoll,
    ) -> Self {
        let sockets = SocketTable::new();

        Self {
            name,
            interface: SpinLock::new(interface),
            used_ports: SpinLock::new(BTreeMap::new()),
            sockets,
            sched_poll,
        }
    }

    pub(super) fn name(&self) -> &str {
        &self.name
    }

    pub(super) fn ipv4_addr(&self) -> Option<Ipv4Address> {
        self.interface.lock().ipv4_addr()
    }

    pub(super) fn sched_poll(&self) -> &E::ScheduleNextPoll {
        &self.sched_poll
    }
}

// Lock order: interface -> sockets
impl<E: Ext> IfaceCommon<E> {
    /// Acquires the lock to the interface.
    pub(crate) fn interface(&self) -> SpinLockGuard<smoltcp::iface::Interface, LocalIrqDisabled> {
        self.interface.lock()
    }

    /// Returns a reference to the socket table (internally synchronized).
    pub(crate) fn sockets(&self) -> &SocketTable<E> {
        &self.sockets
    }
}

const IP_LOCAL_PORT_START: u16 = 32768;
const IP_LOCAL_PORT_END: u16 = 60999;

impl<E: Ext> IfaceCommon<E> {
    pub(super) fn bind(
        &self,
        iface: Arc<dyn Iface<E>>,
        config: BindPortConfig,
    ) -> core::result::Result<BoundPort<E>, BindError> {
        let port = self.bind_port(config)?;
        Ok(BoundPort { iface, port })
    }

    /// Allocates an unused ephemeral port.
    ///
    /// We follow the port range that many Linux kernels use by default, which is 32768-60999.
    ///
    /// See <https://en.wikipedia.org/wiki/Ephemeral_port>.
    fn alloc_ephemeral_port(&self) -> Option<u16> {
        let mut used_ports = self.used_ports.lock();
        for port in IP_LOCAL_PORT_START..=IP_LOCAL_PORT_END {
            if let Entry::Vacant(e) = used_ports.entry(port) {
                e.insert(0);
                return Some(port);
            }
        }
        None
    }

    fn bind_port(&self, config: BindPortConfig) -> Result<u16, BindError> {
        let port = if let Some(port) = config.port() {
            port
        } else {
            match self.alloc_ephemeral_port() {
                Some(port) => port,
                None => return Err(BindError::Exhausted),
            }
        };

        let mut used_ports = self.used_ports.lock();

        if let Some(used_times) = used_ports.get_mut(&port) {
            if *used_times == 0 || config.can_reuse() {
                // FIXME: Check if the previous socket was bound with SO_REUSEADDR.
                *used_times += 1;
            } else {
                return Err(BindError::InUse);
            }
        } else {
            used_ports.insert(port, 1);
        }

        Ok(port)
    }

    /// Releases the port so that it can be used again (if it is not being reused).
    fn release_port(&self, port: u16) {
        let mut used_ports = self.used_ports.lock();
        if let Some(used_times) = used_ports.remove(&port) {
            if used_times != 1 {
                used_ports.insert(port, used_times - 1);
            }
        }
    }
}

impl<E: Ext> IfaceCommon<E> {
    pub(crate) fn register_udp_socket(&self, socket: Arc<UdpSocketBg<E>>) {
        self.sockets.insert_udp_socket(socket);
    }

    pub(crate) fn remove_tcp_listener(&self, socket: &Arc<TcpListenerBg<E>>) {
        let removed = self.sockets.remove_listener(socket);
        debug_assert!(removed.is_some());
    }

    pub(crate) fn remove_udp_socket(&self, socket: &Arc<UdpSocketBg<E>>) {
        let removed = self.sockets.remove_udp_socket(socket);
        debug_assert!(removed.is_some());
    }
}

impl<E: Ext> IfaceCommon<E> {
    pub(super) fn poll<D, P, Q>(
        &self,
        device: &mut D,
        mut process_phy: P,
        mut dispatch_phy: Q,
    ) -> Option<u64>
    where
        D: Device + ?Sized,
        // New ingress processing signature: no Context required.
        P: for<'pkt, 'tx> FnHelper<
            &'pkt [u8],
            D::TxToken<'tx>,
            Option<(Ipv4Packet<&'pkt [u8]>, D::TxToken<'tx>)>,
        >,
        // New egress dispatch signature: no Context required.
        Q: FnMut(&Packet, D::TxToken<'_>),
    {
        ostd::arch::riscv::threadlet::threadlet_syn_print(5, 171);
        // Compute a time snapshot and checksum caps. Access to Context will be
        // provided on demand via short-lived locks.
        let now = get_network_timestamp();
        let caps = device.capabilities().checksum;
        let cx_access = CxAccess { common: self };

        loop {
            ostd::arch::riscv::threadlet::threadlet_syn_print(5, 172);
            let mut new_tcp_conns = Vec::new();

            let mut context = PollContext::new(&cx_access, now, &caps, &self.sockets, &mut new_tcp_conns);
            ostd::arch::riscv::threadlet::threadlet_syn_print(5, 173);
            context.poll_ingress(device, &mut process_phy, &mut dispatch_phy);

            ostd::arch::riscv::threadlet::threadlet_syn_print(5, 173);

            context.poll_egress(device, &mut dispatch_phy);

            ostd::arch::riscv::threadlet::threadlet_syn_print(5, 175);

            // New packets sent by new connections are not handled. So if there are new
            // connections, try again.
            if new_tcp_conns.is_empty() {
                ostd::arch::riscv::threadlet::threadlet_syn_print(5, 176);
                break;
            } else {
                // Insert new connections using table's internal locks.
                new_tcp_conns.into_iter().for_each(|tcp_conn| {
                    let res = self.sockets.insert_connection(tcp_conn);
                    debug_assert!(res.is_ok());
                });
                ostd::arch::riscv::threadlet::threadlet_syn_print(5, 177);
            }
        }

        {
            // Cleanup dead TCP connections, then deliver events using snapshots.
            ostd::arch::riscv::threadlet::threadlet_syn_print(5, 178);
            self.sockets.remove_dead_tcp_connections();

            ostd::arch::riscv::threadlet::threadlet_syn_print(5, 178);

            for socket in self.sockets.tcp_listeners_snapshot() {
                if socket.has_events() {
                    socket.on_events();
                }
            }

            for socket in self.sockets.tcp_connections_snapshot() {
                if socket.has_events() {
                    socket.on_events();
                }
            }

            for socket in self.sockets.udp_sockets_snapshot() {
                if socket.has_events() {
                    socket.on_events();
                }
            }
            ostd::arch::riscv::threadlet::threadlet_syn_print(5, 179);
        }

        // Note that only TCP connections can have timers set, so as far as the time to poll is
        // concerned, we only need to consider TCP connections.
        let next_poll = self.sockets
            .tcp_connections_snapshot()
            .iter()
            .map(|socket| socket.next_poll_at_ms())
            .min();
        ostd::arch::riscv::threadlet::threadlet_syn_print(5, 180);
        next_poll
    }
}

/// A port bound to an iface.
///
/// When dropped, the port is automatically released.
//
// FIXME: TCP and UDP ports are independent. Find a way to track the protocol here.
pub struct BoundPort<E: Ext> {
    iface: Arc<dyn Iface<E>>,
    port: u16,
}

impl<E: Ext> BoundPort<E> {
    /// Returns a reference to the iface.
    pub fn iface(&self) -> &Arc<dyn Iface<E>> {
        &self.iface
    }

    /// Returns the port number.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Returns the bound endpoint.
    pub fn endpoint(&self) -> Option<IpEndpoint> {
        let ip_addr = {
            let ipv4_addr = self.iface().ipv4_addr()?;
            IpAddress::Ipv4(ipv4_addr)
        };
        Some(IpEndpoint::new(ip_addr, self.port))
    }
}

impl<E: Ext> Drop for BoundPort<E> {
    fn drop(&mut self) {
        self.iface.common().release_port(self.port);
    }
}
