// SPDX-License-Identifier: MPL-2.0

use alloc::{borrow::ToOwned, sync::Arc, vec, vec::Vec, format};
use aster_bigtcp::device::WithDevice;
use aster_softirq::Taskless;
use ostd::sync::{LocalIrqDisabled, SpinLock};
use spin::Once;

use super::{poll::poll_ifaces, Iface};
use crate::{net::iface::sched::PollScheduler, prelude::*};
use crate::net::socket::Socket; // bring trait methods like bind/connect into scope
use aster_bigtcp::wire::Ipv4Address;
use crate::net::socket::SocketAddr;
use crate::net::socket::ip::datagram::DatagramSocket;
use spin::Once as SpinOnce;

pub static IFACES: Once<Vec<Arc<Iface>>> = Once::new();

// Short-locking device facade for icenet
#[cfg(target_arch = "riscv64")]
mod facade_icenet {
    use alloc::vec;
    use alloc::vec::Vec;
    use aster_bigtcp::device as smoldev;
    use aster_bigtcp::device::{Device as SmolDevice, RxToken as SmolRxToken, TxToken as SmolTxToken, DeviceCapabilities, NotifyDevice};
    use aster_bigtcp::time::Instant;
    use ostd::mm::VmWriter;
    use ostd::sync::{LocalIrqDisabled, SpinLock};
    use alloc::sync::Arc;
    // Lock-free fast path to check active RX queue availability.
    use aster_network::icenet::active_queue_has_data_fast;

    pub struct DeviceFacade {
        pub inner: Arc<SpinLock<dyn aster_network::AnyNetworkDevice, LocalIrqDisabled>>,
        /// Snapshot of immutable device capabilities to avoid repeated locking.
        caps: DeviceCapabilities,
    }

    impl DeviceFacade {
        pub fn new(inner: Arc<SpinLock<dyn aster_network::AnyNetworkDevice, LocalIrqDisabled>>) -> Self {
            // Short lock only once to cache capabilities; they are static for device lifetime.
            let caps = { let dev = inner.lock(); dev.capabilities() };
            Self { inner, caps }
        }
    }

    pub struct FacadeRxToken(aster_network::RxBuffer);
    pub struct FacadeTxToken { inner: Arc<SpinLock<dyn aster_network::AnyNetworkDevice, LocalIrqDisabled>> }

    impl SmolRxToken for FacadeRxToken {
        fn consume<R, F>(self, f: F) -> R where F: FnOnce(&[u8]) -> R {
            // Mirror existing RxToken implementation to avoid behavioral differences.
            let mut packet = self.0.packet();
            let mut buffer = vec![0u8; packet.remain()];
            packet.read(&mut VmWriter::from(&mut buffer as &mut [u8]));
            #[cfg(irqdebug)]
            {
                let show = core::cmp::min(64, buffer.len());
                let mut ascii_buf = [0u8; 64];
                for i in 0..show { let b = buffer[i]; ascii_buf[i] = if (0x20..=0x7e).contains(&b) { b } else { b'.' }; }
                let ascii = core::str::from_utf8(&ascii_buf[..show]).unwrap_or("");
                ostd::early_println!("[rx] frame: len={} head-ascii='{}'", buffer.len(), ascii);
            }
            f(&buffer)
        }
    }

    impl SmolTxToken for FacadeTxToken {
        fn consume<R, F>(self, len: usize, f: F) -> R where F: FnOnce(&mut [u8]) -> R {
            let mut buffer = vec![0u8; len];
            let res = f(&mut buffer);
            // Do not suppress RX on TX congestion: drop silently if Busy/NotReady.
            match aster_network::icenet::try_send_packet(&buffer) {
                Ok(()) => {}
                Err(aster_network::VirtioNetError::Busy) | Err(aster_network::VirtioNetError::NotReady) => {}
                Err(e) => {
                    #[cfg(netdebug)]
                    {
                        ostd::early_println!("[tx] drop send error: {:?}", e);
                    }
                }
            }
            res
        }
    }

    impl SmolDevice for DeviceFacade {
        type RxToken<'a> = FacadeRxToken;
        type TxToken<'a> = FacadeTxToken;

        fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
            // Do not take global device lock to decide RX availability.
            if !active_queue_has_data_fast() {
                return None;
            }
            match aster_network::icenet::try_receive_one() {
                Some(rx_buffer) => {
                    let tx = FacadeTxToken { inner: self.inner.clone() };
                    Some((FacadeRxToken(rx_buffer), tx))
                }
                None => None,
            }
        }

        fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
            // Always provide a TX token; congestion is handled by dropping in TxToken::consume.
            Some(FacadeTxToken { inner: self.inner.clone() })
        }

        fn capabilities(&self) -> DeviceCapabilities { self.caps.clone() }
    }

    // For icenet, notify_poll_end is a no-op; avoid taking any lock here.
    impl NotifyDevice for DeviceFacade { fn notify_poll_end(&mut self) {} }
}

pub fn init() {
    IFACES.call_once(|| {
        let mut ifaces: Vec<Arc<Iface>> = Vec::new();
        let virtio_idx = if let Some(v) = new_virtio() { let idx = ifaces.len(); ifaces.push(v); Some(idx) } else { None };

        if let Some(icenet) = new_icenet() { let idx = ifaces.len(); ifaces.push(icenet); let _ = idx; }
        let iface_loopback = new_loopback();
        ifaces.push(iface_loopback);


        if let Some(virtio_idx) = virtio_idx {
            // Virtio-Net → poll virtio iface
            let idx = virtio_idx;
            let recv_cb = move || {
                let taskless = Taskless::new(move || {
                    let ifaces = IFACES.get().unwrap();
                    if let Some(iface) = ifaces.get(idx) { iface.poll(); }
                });
                taskless.schedule_urgent();
            };
            aster_network::register_recv_callback(aster_virtio::device::network::DEVICE_NAME, recv_cb);
        }


        ifaces
    });

    poll_ifaces();

    // Debug UDP kernel bindings are disabled to avoid port conflicts with user-space tests.
    // If needed for kernel-only testing, uncomment and ensure no user process binds the same port.
    // ostd::early_println!("[net/debug] installing debug UDP socket 127.0.0.1:11181");
    // debug_bind_udp(127, 0, 0, 1, 11181);
    // ostd::early_println!("[net/debug] installing debug UDP socket 10.0.2.15:5555");
    // debug_bind_udp(10, 0, 2, 15, 5555);
}

fn new_virtio() -> Option<Arc<Iface>> {
    use aster_bigtcp::{
        iface::EtherIface,
        wire::{EthernetAddress, Ipv4Address, Ipv4Cidr},
    };
    use aster_network::AnyNetworkDevice;
    use aster_virtio::device::network::DEVICE_NAME;

    const VIRTIO_ADDRESS: Ipv4Address = Ipv4Address::new(10, 0, 2, 15);
    const VIRTIO_ADDRESS_PREFIX_LEN: u8 = 24; // mask: 255.255.255.0
    const VIRTIO_GATEWAY: Ipv4Address = Ipv4Address::new(10, 0, 2, 2);

    let Some(virtio_net) = aster_network::get_device(DEVICE_NAME) else {
        if cfg!(netdebug) { println!("[net/iface] virtio device not present; skipping iface"); }
        return None;
    };

    let ether_addr = virtio_net.lock().mac_addr().0;
    // Print iface bring-up info unconditionally
    if cfg!(netdebug) { println!(
        "[net/iface] creating iface '{}' mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} ip={}.{}.{}.{}/{} gw={}.{}.{}.{}",
        "virtio",
        ether_addr[0], ether_addr[1], ether_addr[2],
        ether_addr[3], ether_addr[4], ether_addr[5],
        10, 0, 2, 15,
        VIRTIO_ADDRESS_PREFIX_LEN,
        10, 0, 2, 2
    ); }

    struct Wrapper(Arc<SpinLock<dyn AnyNetworkDevice, LocalIrqDisabled>>);

    impl WithDevice for Wrapper {
        type Device = dyn AnyNetworkDevice;

        fn with<F, R>(&self, f: F) -> R
        where
            F: FnOnce(&mut Self::Device) -> R,
        {
            let mut device = self.0.lock();
            f(&mut *device)
        }
    }

    let iface: Arc<Iface> = EtherIface::new(
        Wrapper(virtio_net),
        EthernetAddress(ether_addr),
        Ipv4Cidr::new(VIRTIO_ADDRESS, VIRTIO_ADDRESS_PREFIX_LEN),
        VIRTIO_GATEWAY,
        "virtio".to_owned(),
        PollScheduler::new(),
    );
    if cfg!(netdebug) { println!("[net/iface] smoltcp EtherIface created: {}", iface.name()); }
    Some(iface)
}

fn new_icenet() -> Option<Arc<Iface>> {
    use aster_bigtcp::{
        iface::EtherIface,
        wire::{EthernetAddress, Ipv4Address, Ipv4Cidr},
    };
    use aster_network::AnyNetworkDevice;

    // Default addressing for FPGA host TAP topology (can be overridden by kcmdline module args):
    //   icenet.addr=172.16.0.2/16  icenet.gw=172.16.0.1
    const ICENET_ADDRESS_DFLT: Ipv4Address = Ipv4Address::new(172, 16, 0, 2);
    const ICENET_PREFIX_DFLT: u8 = 16;
    const ICENET_GATEWAY_DFLT: Ipv4Address = Ipv4Address::new(172, 16, 0, 1);

    let Some(icenet_dev) = aster_network::get_device("icenet") else { return None; };

    // Parse optional overrides from kernel command line:
    let (mut addr, mut prefix_len, mut gateway) = (ICENET_ADDRESS_DFLT, ICENET_PREFIX_DFLT, ICENET_GATEWAY_DFLT);
    {
        use ostd::boot::{kcmdline::ModuleArg, kernel_cmdline};

        fn parse_ipv4(s: &str) -> Option<Ipv4Address> {
            let mut parts = [0u8; 4];
            let segs: alloc::vec::Vec<&str> = s.split('.').collect();
            if segs.len() != 4 { return None; }
            for (i, p) in segs.iter().enumerate() {
                let v = p.parse::<u8>().ok()?;
                parts[i] = v;
            }
            Some(Ipv4Address::new(parts[0], parts[1], parts[2], parts[3]))
        }
        fn parse_cidr(s: &str) -> Option<(Ipv4Address, u8)> {
            let mut it = s.split('/');
            let ip_s = it.next()?;
            let len_s = it.next()?;
            if it.next().is_some() { return None; }
            let ip = parse_ipv4(ip_s)?;
            let plen = len_s.parse::<u8>().ok()?;
            if plen > 32 { return None; }
            Some((ip, plen))
        }

        if let Some(args) = kernel_cmdline().get_module_args("icenet") {
            for a in args {
                match a {
                    ModuleArg::KeyVal(name, value) if name.as_bytes() == b"addr" => {
                        if let Ok(v) = core::str::from_utf8(value.as_bytes()) {
                            if let Some((ip, plen)) = parse_cidr(v) { addr = ip; prefix_len = plen; }
                        }
                    }
                    ModuleArg::KeyVal(name, value) if name.as_bytes() == b"gw" => {
                        if let Ok(v) = core::str::from_utf8(value.as_bytes()) {
                            if let Some(ip) = parse_ipv4(v) { gateway = ip; }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    let ether_addr = icenet_dev.lock().mac_addr().0;
    if cfg!(netdebug) {
        println!(
            "[net/iface] creating iface '{}' mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} ip={}/{} gw={}",
            "icenet",
            ether_addr[0], ether_addr[1], ether_addr[2],
            ether_addr[3], ether_addr[4], ether_addr[5],
            addr,
            prefix_len,
            gateway
        );
    }

    struct Wrapper(Arc<SpinLock<dyn AnyNetworkDevice, LocalIrqDisabled>>);
    impl WithDevice for Wrapper {
        // Use short-locking facade for icenet only.
        type Device = crate::net::iface::init::facade_icenet::DeviceFacade;
        fn with<F, R>(&self, f: F) -> R
        where F: FnOnce(&mut Self::Device) -> R,
        {
            let mut facade = crate::net::iface::init::facade_icenet::DeviceFacade::new(self.0.clone());
            f(&mut facade)
        }
    }

    let iface: Arc<Iface> = EtherIface::new(
        Wrapper(icenet_dev),
        EthernetAddress(ether_addr),
        Ipv4Cidr::new(addr, prefix_len),
        gateway,
        "icenet".to_owned(),
        PollScheduler::new(),
    );
    if cfg!(netdebug) { println!("[net/iface] smoltcp EtherIface created: {}", iface.name()); }
    // Debug: confirm iface bring-up regardless of netdebug prints
    ostd::early_println!(
        "[net/iface] icenet iface ready name='{}' ipv4={}/{} gw={}",
        iface.name(), addr, prefix_len, gateway
    );


    let iface_cloned = iface.clone();
    let recv_cb = move || {
        iface_cloned.poll();
    };
    aster_network::register_recv_callback("icenet", recv_cb);
    ostd::early_println!("[net/iface] icenet recv callback registered");

    // Proactively announce our address on the LAN to help the host learn it.
    // Do this after enabling RX interrupts (register_recv_callback above will enable RX for icenet).
    #[cfg(target_arch = "riscv64")]
    {
        ostd::early_println!("[net/iface] icenet send gratuitous ARP");
        aster_network::icenet::send_gratuitous_arp();
    }

    Some(iface)
}

fn new_loopback() -> Arc<Iface> {
    use aster_bigtcp::{
        device::{Loopback, Medium},
        iface::IpIface,
        wire::{Ipv4Address, Ipv4Cidr},
    };

    const LOOPBACK_ADDRESS: Ipv4Address = Ipv4Address::new(127, 0, 0, 1);
    const LOOPBACK_ADDRESS_PREFIX_LEN: u8 = 8; // mask: 255.0.0.0

    struct Wrapper(Mutex<Loopback>);

    impl WithDevice for Wrapper {
        type Device = Loopback;

        fn with<F, R>(&self, f: F) -> R
        where
            F: FnOnce(&mut Self::Device) -> R,
        {
            let mut device = self.0.lock();
            f(&mut device)
        }
    }

    IpIface::new(
        Wrapper(Mutex::new(Loopback::new(Medium::Ip))),
        Ipv4Cidr::new(LOOPBACK_ADDRESS, LOOPBACK_ADDRESS_PREFIX_LEN),
        "lo".to_owned(),
        PollScheduler::new(),
    ) as _
}

// Keep debug UDP sockets alive
static DEBUG_UDP_SOCKS: SpinOnce<SpinLock<Vec<Arc<DatagramSocket>>, LocalIrqDisabled>> = SpinOnce::new();

fn debug_bind_udp(a: u8, b: u8, c: u8, d: u8, port: u16) {
    // Create and bind a UDP socket to the specified IPv4:port so that
    // `echo -n "..." | nc -u -w1 a.b.c.d port` inside the guest will
    // hit our kernel UDP path and trigger debug prints.
    let sock = DatagramSocket::new(false);
    let addr = SocketAddr::IPv4(Ipv4Address::new(a, b, c, d), port);
    match sock.bind(addr) {
        Ok(()) => {
            DEBUG_UDP_SOCKS.call_once(|| SpinLock::new(Vec::new()));
            if let Some(list) = DEBUG_UDP_SOCKS.get() {
                list.lock().push(sock.clone());
            }
            ostd::early_println!(
                "[net/debug] bound UDP listener at {}.{}.{}.{}:{}",
                a, b, c, d, port
            );
        }
        Err(e) => {
            ostd::early_println!(
                "[net/debug] failed to bind UDP {}.{}.{}.{}:{}: {:?}",
                a, b, c, d, port, e
            );
        }
    }
}
