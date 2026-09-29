// SPDX-License-Identifier: MPL-2.0

#![no_std]
// #![deny(unsafe_code)]
#![feature(trait_alias)]
#![feature(fn_traits)]
#![feature(linked_list_cursors)]

mod buffer;
pub mod dma_pool;
mod driver;
#[cfg(target_arch = "riscv64")]
pub mod icenet;
pub mod stats;

extern crate alloc;

use alloc::{collections::BTreeMap, string::String, sync::Arc, vec::Vec};
use core::{any::Any, fmt::Debug};

use aster_bigtcp::device::DeviceCapabilities;
pub use buffer::{RxBuffer, TxBuffer, RX_BUFFER_POOL, TX_BUFFER_LEN};
use component::{init_component, ComponentInitError};
pub use dma_pool::DmaSegment;
use ostd::{
    sync::{LocalIrqDisabled, SpinLock},
    Pod,
};
use spin::Once;

#[derive(Debug, Clone, Copy, Pod)]
#[repr(C)]
pub struct EthernetAddr(pub [u8; 6]);

#[derive(Debug, Clone, Copy)]
pub enum VirtioNetError {
    NotReady,
    WrongToken,
    Busy,
    Unknown,
}

pub trait AnyNetworkDevice: Send + Sync + Any + Debug {
    // ================Device Information=================

    fn mac_addr(&self) -> EthernetAddr;
    fn capabilities(&self) -> DeviceCapabilities;

    // ================Device Operation===================

    fn can_receive(&self) -> bool;
    fn can_send(&self) -> bool;

    /// Receives a packet from network. If packet is ready, returns a `RxBuffer` containing the packet.
    /// Otherwise, return [`VirtioNetError::NotReady`].
    fn receive(&mut self) -> Result<RxBuffer, VirtioNetError>;

    /// Sends a packet to network.
    fn send(&mut self, packet: &[u8]) -> Result<(), VirtioNetError>;

    /// Frees processes tx buffers.
    fn free_processed_tx_buffers(&mut self);

    /// Notifies the device driver that a polling operation has ended.
    ///
    /// The driver can assume that the device remains protected by acquiring a poll lock
    /// for the entire duration of the polling process.
    /// Thus two polling process cannot happen simultaneously.
    fn notify_poll_end(&mut self);
}

pub trait NetDeviceIrqHandler = Fn() + Send + Sync + 'static;

pub fn register_device(
    name: String,
    device: Arc<SpinLock<dyn AnyNetworkDevice, LocalIrqDisabled>>,
) {
    COMPONENT
        .get()
        .unwrap()
        .network_device_table
        .lock()
        .insert(name.clone(), NetworkDeviceIrqCallbackSet::new(device));
    // Debug: confirm device registration path
    ostd::early_println!("[net-dev] register_device name='{}'", name);
}

pub fn get_device(str: &str) -> Option<Arc<SpinLock<dyn AnyNetworkDevice, LocalIrqDisabled>>> {
    let table = COMPONENT.get().unwrap().network_device_table.lock();
    let callbacks = table.get(str)?;
    Some(callbacks.device.clone())
}

/// Registers callback which will be called when receiving message.
///
/// Since the callback will be called in interrupt context,
/// the callback function should NOT sleep.
pub fn register_recv_callback(name: &str, callback: impl NetDeviceIrqHandler) {
    let device_table = COMPONENT.get().unwrap().network_device_table.lock();
    let Some(callbacks) = device_table.get(name) else {
        return;
    };
    let mut list = callbacks.recv_callbacks.lock();
    let is_first = list.is_empty();
    list.push(Arc::new(callback));
    // Debug: show total recv callbacks registered for the device
    ostd::early_println!(
        "[net-irq] register_recv_callback name='{}' total_recv_cbs={}",
        name,
        list.len()
    );

    // If this is the first callback for icenet, enable its interrupts.
    if name == "icenet" && is_first {
        #[cfg(target_arch = "riscv64")]
        ostd::early_println!("[net-irq] enable icenet RX interrupts");
        icenet::enable_rx_interrupts();
    }
}

/// Registers a per-queue recv callback for a device.
/// The callback will be invoked only when `handle_recv_irq_queue(name, q)` is called.
// NOTE: Per-queue registration can be achieved by giving unique device names
// (e.g., "icenet0-q0", "icenet0-q1") and using `register_recv_callback`.

pub fn register_send_callback(name: &str, callback: impl NetDeviceIrqHandler) {
    let device_table = COMPONENT.get().unwrap().network_device_table.lock();
    let Some(callbacks) = device_table.get(name) else {
        return;
    };
    callbacks.send_callbacks.lock().push(Arc::new(callback));
}

pub fn handle_recv_irq(name: &str) {
    let device_table = COMPONENT.get().unwrap().network_device_table.lock();
    let Some(callbacks) = device_table.get(name) else {
        return;
    };

    // Merge TX completion handling into the RX IRQ path (NAPI-like):
    // free processed TX buffers proactively to reduce the need for a separate
    // send IRQ callback/registration.
    {
        let mut device = callbacks.device.lock();
        device.free_processed_tx_buffers();
    }

    // Directly invoke recv callbacks (hardirq context). If this becomes heavy,
    // we can introduce a dedicated network softirq without creating dependency cycles.
    let callbacks = callbacks.recv_callbacks.lock();

    for callback in callbacks.iter() {
        callback();
    }
}

/// Per-queue variant: only dispatch the callbacks registered for the given queue `q`.
// NOTE: Per-queue dispatch is realized by calling `handle_recv_irq` with the
// per-queue device name.

pub fn handle_send_irq(name: &str) {
    if cfg!(netdebug) { ostd::early_println!("[net-irq] send IRQ from device: {}", name); }
    let device_table = COMPONENT.get().unwrap().network_device_table.lock();
    let Some(callbacks) = device_table.get(name) else {
        return;
    };

    let can_send = {
        let mut device = callbacks.device.lock();
        device.free_processed_tx_buffers();
        device.can_send()
    };
    if !can_send {
        return;
    }

    let callbacks = callbacks.send_callbacks.lock();
    for callback in callbacks.iter() {
        callback();
    }
}

pub fn all_devices() -> Vec<(String, NetworkDeviceRef)> {
    let network_devs = COMPONENT.get().unwrap().network_device_table.lock();
    network_devs
        .iter()
        .map(|(name, callbacks)| (name.clone(), callbacks.device.clone()))
        .collect()
}

static COMPONENT: Once<Component> = Once::new();
pub(crate) static NETWORK_IRQ_HANDLERS: Once<
    SpinLock<Vec<Arc<dyn NetDeviceIrqHandler>>, LocalIrqDisabled>,
> = Once::new();

#[init_component]
fn init() -> Result<(), ComponentInitError> {
    let a = Component::init()?;
    COMPONENT.call_once(|| a);
    NETWORK_IRQ_HANDLERS.call_once(|| SpinLock::new(Vec::new()));
    buffer::init();
    Ok(())
}

type NetDeviceIrqHandlerListRef =
    Arc<SpinLock<Vec<Arc<dyn NetDeviceIrqHandler>>, LocalIrqDisabled>>;
type NetworkDeviceRef = Arc<SpinLock<dyn AnyNetworkDevice, LocalIrqDisabled>>;

struct Component {
    /// Device list, the key is device name, value is (callbacks, device);
    network_device_table: SpinLock<BTreeMap<String, NetworkDeviceIrqCallbackSet>, LocalIrqDisabled>,
}

/// The send callbacks and recv callbacks for a network device
struct NetworkDeviceIrqCallbackSet {
    device: NetworkDeviceRef,
    recv_callbacks: NetDeviceIrqHandlerListRef,
    send_callbacks: NetDeviceIrqHandlerListRef,
}

impl NetworkDeviceIrqCallbackSet {
    fn new(device: NetworkDeviceRef) -> Self {
        Self {
            device,
            recv_callbacks: Arc::new(SpinLock::new(Vec::new())),
            send_callbacks: Arc::new(SpinLock::new(Vec::new())),
        }
    }
}

impl Component {
    pub fn init() -> Result<Self, ComponentInitError> {
        Ok(Self {
            network_device_table: SpinLock::new(BTreeMap::new()),
        })
    }
}
#[cfg(target_arch = "riscv64")]
pub fn trigger_test_send() {
    let Some(dev) = get_device("icenet") else {
        if cfg!(netdebug) { ostd::early_println!("[icenet-test] no icenet device registered"); }
        return;
    };
    // get device MAC 
    let src = dev.lock().mac_addr().0;
    let mut frame = [0u8; 60];
    // target MAC: broadcast
    frame[0..6].copy_from_slice(&[0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);
    // source MAC
    frame[6..12].copy_from_slice(&src);
    // EtherType: IPv4 (0x0800)
    frame[12] = 0x08;
    frame[13] = 0x00;
    // payload 
    for b in &mut frame[14..] { *b = 0xAB; }

    let res = {
        let mut d = dev.lock();
        d.send(&frame)
    };
    if cfg!(netdebug) {
        match res {
            Ok(()) => ostd::early_println!(
                "[icenet-test] TX test frame submitted len={} src={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                frame.len(), src[0], src[1], src[2], src[3], src[4], src[5]
            ),
            Err(e) => ostd::early_println!("[icenet-test] TX submit failed: {:?}", e),
        }
    }
}
