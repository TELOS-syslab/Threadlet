// SPDX-License-Identifier: MPL-2.0

use alloc::{
    boxed::Box, collections::linked_list::LinkedList, string::ToString, sync::Arc, vec, vec::Vec,
};
use core::{fmt::Debug, mem::size_of};

use aster_bigtcp::device::{Checksum, DeviceCapabilities, Medium};
use aster_network::{
    AnyNetworkDevice, EthernetAddr, RxBuffer, TxBuffer, VirtioNetError, RX_BUFFER_POOL,
};
use aster_util::slot_vec::SlotVec;
use log::{debug, warn};
use ostd::{
    mm::DmaStream,
    sync::{LocalIrqDisabled, SpinLock},
    trap::TrapFrame,
};
// Bring `Pod` trait into scope to use `as_bytes()` on `VirtioNetHdr`.
use ostd::Pod;

use super::{config::VirtioNetConfig, header::VirtioNetHdr};
use crate::{
    device::{network::config::NetworkFeatures, VirtioDeviceError},
    queue::{QueueError, VirtQueue},
    transport::{ConfigManager, VirtioTransport},
};

struct QueuePair {
    send_queue: VirtQueue,
    recv_queue: VirtQueue,
    tx_buffers: Vec<Option<TxBuffer>>, // size = QUEUE_SIZE
    rx_buffers: SlotVec<RxBuffer>,
    poll_stat: PollStatistics,
}

pub struct NetworkDevice {
    config_manager: ConfigManager<VirtioNetConfig>,
    // For smoltcp use
    caps: DeviceCapabilities,
    mac_addr: EthernetAddr,
    // Since the virtio net header remains consistent for each sending packet,
    // we store it to avoid recreating the header repeatedly.
    header: VirtioNetHdr,
    // Multiple RX/TX queues (pairs). For mmio, interrupts are shared; for pci, can be per-queue.
    queue_pairs: Vec<QueuePair>,
    transport: Box<dyn VirtioTransport>,
    /// Actual virtio-net header length on RX/TX buffers (depends on features)
    virtio_hdr_len: usize,
}

/// Structure to track the number of packets sent and received during a single polling process.
struct PollStatistics {
    sent_packet: usize,
    received_packet: usize,
}

impl PollStatistics {
    const fn new() -> Self {
        Self {
            sent_packet: 0,
            received_packet: 0,
        }
    }
}

impl NetworkDevice {
    pub(crate) fn negotiate_features(device_features: u64) -> u64 {
        let device_features = NetworkFeatures::from_bits_truncate(device_features);
        let supported_features = NetworkFeatures::support_features();
        let network_features = device_features & supported_features;

        if network_features != device_features {
            warn!(
                "Virtio net contains unsupported device features: {:?}",
                device_features.difference(supported_features)
            );
        }

        debug!("{:?}", network_features);
        network_features.bits()
    }

    pub fn init(mut transport: Box<dyn VirtioTransport>) -> Result<(), VirtioDeviceError> {
        let config_manager = VirtioNetConfig::new_manager(transport.as_ref());
        let config = config_manager.read_config();
        debug!("virtio_net_config = {:?}", config);
        if cfg!(netdebug) {
            ostd::early_println!(
                "[virtio-net] config: mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} mtu={}",
                config.mac.0[0], config.mac.0[1], config.mac.0[2],
                config.mac.0[3], config.mac.0[4], config.mac.0[5], config.mtu
            );
        }
        let mac_addr = config.mac;
        let features = NetworkFeatures::from_bits_truncate(Self::negotiate_features(
            transport.read_device_features(),
        ));
        debug!("features = {:?}", features);
        if cfg!(netdebug) {
            ostd::early_println!("[virtio-net] negotiated features: {:?}", features);
        }

        let caps = init_caps(&features, &config);
        // Decide virtio-net header length: base 10 bytes; +2 bytes num_buffers if MRG_RXBUF or modern PCI
        let has_mrg_rxbuf = features.contains(NetworkFeatures::VIRTIO_NET_F_MRG_RXBUF);
        let is_modern = !transport.is_legacy_version();
        let virtio_hdr_len: usize = if has_mrg_rxbuf || is_modern { 12 } else { 10 };

        // Determine queue pairs: prefer device-advertised MQ, fallback to 1.
        let mut num_pairs = 1usize;
        if features.contains(NetworkFeatures::VIRTIO_NET_F_MQ) {
            let pairs_from_cfg = core::cmp::max(1, config.max_virtqueue_pairs() as usize);
            let pairs_from_transport = (transport.num_queues() as usize) / 2;
            num_pairs = core::cmp::min(pairs_from_cfg, core::cmp::max(1, pairs_from_transport));
        }

        if cfg!(netdebug) {
            ostd::early_println!(
                "[virtio-net] queue pairs={} (transport.num_queues()={})",
                num_pairs,
                transport.num_queues()
            );
        }

        let mut queue_pairs: Vec<QueuePair> = Vec::with_capacity(num_pairs);
        for i in 0..num_pairs {
            let rx_idx = (2 * i) as u16;
            let tx_idx = (2 * i + 1) as u16;

            let mut send_queue =
                VirtQueue::new(tx_idx, QUEUE_SIZE, transport.as_mut()).expect("create tx q");
            // Defer enabling TX IRQ until need
            send_queue.disable_callback();

            let mut recv_queue =
                VirtQueue::new(rx_idx, QUEUE_SIZE, transport.as_mut()).expect("create rx q");

            if cfg!(netdebug) {
                ostd::early_println!(
                    "[virtio-net] qpair#{}: rx_idx={} rx.size={} tx_idx={} tx.size={}",
                    i,
                    rx_idx,
                    recv_queue.size(),
                    tx_idx,
                    send_queue.size()
                );
            }

            let tx_buffers: Vec<Option<TxBuffer>> = core::iter::repeat_with(|| None)
                .take(send_queue.size() as usize)
                .collect();
            let mut rx_buffers = SlotVec::new();
            for t in 0..QUEUE_SIZE {
                let rx_pool = RX_BUFFER_POOL.get().unwrap();
                let rx_buffer = RxBuffer::new(virtio_hdr_len, rx_pool);
                let token = recv_queue.add_dma_buf(&[], &[&rx_buffer])?;
                assert_eq!(t, token);
                assert_eq!(rx_buffers.put(rx_buffer) as u16, t);
            }

            if recv_queue.should_notify() {
                debug!("notify receive queue pair#{}", i);
                recv_queue.notify();
            }

            queue_pairs.push(QueuePair {
                send_queue,
                recv_queue,
                tx_buffers,
                rx_buffers,
                poll_stat: PollStatistics::new(),
            });
        }

        let mut device = Self {
            config_manager,
            caps,
            mac_addr,
            header: VirtioNetHdr::default(),
            queue_pairs,
            transport,
            virtio_hdr_len,
        };

        /// Interrupt handler if network device config space changes
        fn config_space_change(_: &TrapFrame) {
            debug!("network device config space change");
        }

        /// Interrupt handlers if network device receives/sends some packet
        fn handle_send_event(_: &TrapFrame) {
            if cfg!(netdebug) { ostd::early_println!("[irq] virtio-net: send IRQ"); }
            aster_network::handle_send_irq(super::DEVICE_NAME);
        }
        fn handle_recv_event(_: &TrapFrame) {
            if cfg!(netdebug) { ostd::early_println!("[irq] virtio-net: recv IRQ"); }
            aster_network::handle_recv_irq(super::DEVICE_NAME);
        }

        device
            .transport
            .register_cfg_callback(Box::new(config_space_change))
            .unwrap();
        // For mmio, index is ignored and interrupts are shared; for pci, these indices
        // are used to route to per-queue vectors. We register only the receive
        // callback and merge TX completion handling into RX path to avoid
        // duplicate polls (NAPI-like behavior).
        device
            .transport
            .register_queue_callback(0, Box::new(handle_recv_event), true)
            .unwrap();

        device.transport.finish_init();
        if cfg!(netdebug) { ostd::early_println!("[virtio-net] transport finished, registering device"); }

        aster_network::register_device(
            super::DEVICE_NAME.to_string(),
            Arc::new(SpinLock::new(device)),
        );
        if cfg!(netdebug) { ostd::early_println!("[virtio-net] device registered to aster_network as '{}'", super::DEVICE_NAME); }
        Ok(())
    }

    /// Adds a `RxBuffer` to the receive queue of pair `idx`.
    fn add_rx_buffer_on(&mut self, idx: usize, rx_buffer: RxBuffer) -> Result<(), VirtioNetError> {
        let pair = &mut self.queue_pairs[idx];
        let token = pair
            .recv_queue
            .add_dma_buf(&[], &[&rx_buffer])
            .map_err(queue_to_network_error)?;
        assert!(pair.rx_buffers.put_at(token as usize, rx_buffer).is_none());

        pair.poll_stat.received_packet += 1;

        if pair.poll_stat.received_packet == QUEUE_SIZE as _ {
            self.notify_receive_queue_on(idx);
        }
        Ok(())
    }

    /// Receives a packet from network.
    fn receive(&mut self) -> Result<RxBuffer, VirtioNetError> {
        // Try each RX queue pair to find available packet
        for i in 0..self.queue_pairs.len() {
            // Pop from used ring (limit the mutable borrow scope)
            let (token, len) = match self.queue_pairs[i].recv_queue.pop_used() {
                Ok(v) => v,
                Err(_) => continue,
            };
            if cfg!(netdebug) {
                ostd::early_println!("[rx] qpair={} token={} len={}", i, token, len);
            }
            // Remove buffer from RX pool of this queue
            let mut rx_buffer = self.queue_pairs[i]
                .rx_buffers
                .remove(token as usize)
                .ok_or(VirtioNetError::WrongToken)?;
            rx_buffer.set_packet_len(len as usize - self.virtio_hdr_len);
            // Refill one buffer on this queue
            let rx_pool = RX_BUFFER_POOL.get().unwrap();
            let new_rx_buffer = RxBuffer::new(self.virtio_hdr_len, rx_pool);
            self.add_rx_buffer_on(i, new_rx_buffer)?;
            return Ok(rx_buffer);
        }
        Err(VirtioNetError::NotReady)
    }

    /// Sends a packet to network.
    fn send(&mut self, packet: &[u8]) -> Result<(), VirtioNetError> {
        if !self.can_send() {
            return Err(VirtioNetError::Busy);
        }

        // Choose a TX queue via software RSS (5-tuple hash), fallback to first-available.
        let mut qi = if self.queue_pairs.len() > 1 {
            match parse_ipv4_5tuple_from_eth(packet) {
                Some((src, dst, proto, sport, dport)) => {
                    let h = rss_hash_v4(src, dst, proto, sport, dport);
                    let idx = (h as usize) % self.queue_pairs.len();
                    if cfg!(netdebug) {
                        ostd::early_println!(
                            "[tx-select] flow={}.{}.{}.{}:{} -> {}.{}.{}.{}:{} proto={} hash=0x{:08x} qpair={}",
                            src[0], src[1], src[2], src[3], sport,
                            dst[0], dst[1], dst[2], dst[3], dport,
                            proto,
                            h,
                            idx
                        );
                    }
                    idx
                }
                None => {
                    // Fallback: simple round policy by scanning availability later
                    usize::MAX
                }
            }
        } else {
            0
        };

        // Ensure chosen queue has available descriptors; otherwise fallback to first-available.
        if qi == usize::MAX || self.queue_pairs[qi].send_queue.available_desc() == 0 {
            qi = self
                .queue_pairs
                .iter()
                .position(|p| p.send_queue.available_desc() >= 1)
                .ok_or(VirtioNetError::Busy)?;
        }

        // Build header with the actual virtio-net header length expected by the device
        let full_hdr = self.header.as_bytes();
        let hdr_slice = &full_hdr[..self.virtio_hdr_len];
        let tx_buffer = TxBuffer::new_raw(hdr_slice, packet, &TX_BUFFER_POOL);

        // Submit to the chosen queue (limit the mutable borrow scope)
        let (token, avail_after) = {
            let pair = &mut self.queue_pairs[qi];
            let token = pair
                .send_queue
                .add_dma_buf(&[&tx_buffer], &[])
                .map_err(queue_to_network_error)?;
            pair.poll_stat.sent_packet += 1;
            debug_assert!(pair.tx_buffers[token as usize].is_none());
            pair.tx_buffers[token as usize] = Some(tx_buffer);
            (token, pair.send_queue.available_desc())
        };

        if cfg!(netdebug) {
            ostd::early_println!(
                "[tx] qpair={} token={} len={} avail_desc={}",
                qi,
                token,
                packet.len(),
                avail_after
            );
        }

        // Notify if the queue is full now
        if avail_after == 0 {
            self.notify_send_queue_on(qi);
        }

        // Reap completions across queues
        self.free_processed_tx_buffers();

        if !self.can_send() {
            self.queue_pairs[qi].send_queue.enable_callback();
        } else {
            self.queue_pairs[qi].send_queue.disable_callback();
        }

        Ok(())
    }

    fn notify_send_queue_on(&mut self, qi: usize) {
        let pair = &mut self.queue_pairs[qi];
        if pair.poll_stat.sent_packet == 0 {
            return;
        }
        if cfg!(netdebug) {
            ostd::early_println!(
                "[notify] qpair={} send: sent {} packets",
                qi, pair.poll_stat.sent_packet
            );
        }
        if pair.send_queue.should_notify() {
            pair.send_queue.notify();
        }
        pair.poll_stat.sent_packet = 0;
    }

    fn notify_receive_queue_on(&mut self, qi: usize) {
        let pair = &mut self.queue_pairs[qi];
        if pair.poll_stat.received_packet == 0 {
            return;
        }
        if cfg!(netdebug) {
            ostd::early_println!(
                "[notify] qpair={} recv: received {} packets",
                qi, pair.poll_stat.received_packet
            );
        }
        if pair.recv_queue.should_notify() {
            pair.recv_queue.notify();
        }
        pair.poll_stat.received_packet = 0;
    }
}

fn queue_to_network_error(err: QueueError) -> VirtioNetError {
    match err {
        QueueError::NotReady => VirtioNetError::NotReady,
        QueueError::WrongToken => VirtioNetError::WrongToken,
        QueueError::BufferTooSmall => VirtioNetError::Busy,
        _ => VirtioNetError::Unknown,
    }
}

fn init_caps(features: &NetworkFeatures, config: &VirtioNetConfig) -> DeviceCapabilities {
    let mut caps = DeviceCapabilities::default();

    caps.max_burst_size = None;
    caps.medium = Medium::Ethernet;

    if features.contains(NetworkFeatures::VIRTIO_NET_F_MTU) {
        // If `VIRTIO_NET_F_MTU` is negotiated, the MTU is decided by the device.
        caps.max_transmission_unit = config.mtu as usize;
    } else {
        // We do not support these features,
        // so this asserts that they are _not_ negotiated.
        //
        // Without these features, the MTU is 1514 bytes per the virtio-net specification
        // (see "5.1.6.3 Setting Up Receive Buffers" and "5.1.6.2 Packet Transmission").
        assert!(
            !features.contains(NetworkFeatures::VIRTIO_NET_F_GUEST_TSO4)
                && !features.contains(NetworkFeatures::VIRTIO_NET_F_GUEST_TSO6)
                && !features.contains(NetworkFeatures::VIRTIO_NET_F_GUEST_UFO)
        );
        caps.max_transmission_unit = 1514;
    }

    // We do not support checksum offloading.
    // So the features must not be negotiated,
    // and we must deliver fully checksummed packets to the device
    // and validate all checksums for packets from the device.
    assert!(
        !features.contains(NetworkFeatures::VIRTIO_NET_F_CSUM)
            && !features.contains(NetworkFeatures::VIRTIO_NET_F_GUEST_CSUM)
    );
    caps.checksum.tcp = Checksum::Both;
    caps.checksum.udp = Checksum::Both;
    caps.checksum.ipv4 = Checksum::Both;
    caps.checksum.icmpv4 = Checksum::Both;

    caps
}

impl AnyNetworkDevice for NetworkDevice {
    fn mac_addr(&self) -> EthernetAddr {
        self.mac_addr
    }

    fn capabilities(&self) -> DeviceCapabilities {
        self.caps.clone()
    }

    fn can_receive(&self) -> bool {
        self.queue_pairs.iter().any(|p| p.recv_queue.can_pop())
    }

    fn can_send(&self) -> bool {
        self.queue_pairs
            .iter()
            .any(|p| p.send_queue.available_desc() >= 1)
    }

    fn receive(&mut self) -> Result<RxBuffer, VirtioNetError> {
        self.receive()
    }

    fn send(&mut self, packet: &[u8]) -> Result<(), VirtioNetError> {
        self.send(packet)
    }

    fn free_processed_tx_buffers(&mut self) {
        for (i, pair) in self.queue_pairs.iter_mut().enumerate() {
            while let Ok((token, _)) = pair.send_queue.pop_used() {
                pair.tx_buffers[token as usize] = None;
                if cfg!(netdebug) {
                    ostd::early_println!("[tx-free] qpair={} token={} freed", i, token);
                }
            }
        }
    }

    fn notify_poll_end(&mut self) {
        for i in 0..self.queue_pairs.len() {
            self.notify_send_queue_on(i);
            self.notify_receive_queue_on(i);
        }
    }
}

impl Debug for NetworkDevice {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("NetworkDevice")
            .field("config", &self.config_manager.read_config())
            .field("mac_addr", &self.mac_addr)
            .field("queue_pairs_len", &self.queue_pairs.len())
            .field("transport", &self.transport)
            .finish()
    }
}

static TX_BUFFER_POOL: SpinLock<LinkedList<DmaStream>, LocalIrqDisabled> =
    SpinLock::new(LinkedList::new());

const QUEUE_RECV: u16 = 0; // kept for compatibility, not used in multi-q path
const QUEUE_SEND: u16 = 1; // kept for compatibility, not used in multi-q path

const QUEUE_SIZE: u16 = 64;

// --------------------
// Software RSS helpers
// --------------------

#[inline]
fn parse_ipv4_5tuple_from_eth(
    frame: &[u8],
) -> Option<([u8; 4], [u8; 4], u8, u16, u16)> {
    // Ethernet header: 14 bytes (dest[6], src[6], ethertype[2])
    if frame.len() < 14 {
        return None;
    }
    let mut off = 12;
    let mut ethertype = u16::from_be_bytes([frame[off], frame[off + 1]]);
    off += 2;
    // Handle single VLAN tag (0x8100)
    if ethertype == 0x8100 {
        if frame.len() < 18 {
            return None;
        }
        ethertype = u16::from_be_bytes([frame[off + 2], frame[off + 3]]);
        off += 4; // skip TCI(2) + encapsulated ethertype(2)
    }
    if ethertype != 0x0800 {
        // Not IPv4
        return None;
    }
    // IPv4 header starts at `off`
    if frame.len() < off + 20 {
        return None;
    }
    let ihl = (frame[off] & 0x0f) as usize * 4;
    if ihl < 20 || frame.len() < off + ihl {
        return None;
    }
    let proto = frame[off + 9];
    let src = [frame[off + 12], frame[off + 13], frame[off + 14], frame[off + 15]];
    let dst = [frame[off + 16], frame[off + 17], frame[off + 18], frame[off + 19]];
    let l4off = off + ihl;
    if frame.len() < l4off + 4 {
        return None;
    }
    // TCP(6) / UDP(17)
    match proto {
        6 | 17 => {
            let sport = u16::from_be_bytes([frame[l4off], frame[l4off + 1]]);
            let dport = u16::from_be_bytes([frame[l4off + 2], frame[l4off + 3]]);
            Some((src, dst, proto, sport, dport))
        }
        _ => None,
    }
}

#[inline]
fn rss_hash_v4(src: [u8; 4], dst: [u8; 4], proto: u8, sport: u16, dport: u16) -> u32 {
    // Simple FNV-1a 32-bit over 5-tuple as bytes (sufficient for queue selection)
    let mut h: u32 = 0x811C_9DC5;
    const FNV_PRIME: u32 = 0x0100_0193;
    let mut feed = |b: u8| {
        h ^= b as u32;
        h = h.wrapping_mul(FNV_PRIME);
    };
    for b in src { feed(b); }
    for b in dst { feed(b); }
    feed(proto);
    for b in sport.to_be_bytes() { feed(b); }
    for b in dport.to_be_bytes() { feed(b); }
    h
}
