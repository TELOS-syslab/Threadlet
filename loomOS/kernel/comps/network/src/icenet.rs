// SPDX-License-Identifier: MPL-2.0

use alloc::{
    collections::{LinkedList, VecDeque},
    sync::Arc,
    vec::Vec,
};
use ostd::{
    Pod, arch::threadlet, io_mem::IoMem, sync::{LocalIrqDisabled, SpinLock}
};
use ostd::mm::{VmIoOnce, HasDaddr, DmaStream, VmWriter, VmReader};
use ostd::io_mem::{mmio_wmb, mmio_rmb};
use aster_softirq::{SoftIrqLine, softirq_id::NIC_SOFTIRQ_BASE};
use ostd::cpu_local_cell;
use core::cell::UnsafeCell;
use core::{sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering}, usize};
use spin::Once as SpinOnce;

use crate::{ AnyNetworkDevice, EthernetAddr, RxBuffer, VirtioNetError, RX_BUFFER_POOL };
use crate::dma_pool::{DmaPool, DmaSegment};
use crate::buffer::TxBuffer;

const DEVICE_NAME: &str = "icenet";
pub const NUM_CORES: usize = 2;     
const RX_RING_DEPTH: usize = 50;
const DOORBELL_CACHELINE_BYTES: usize = 64;
const ETH_HEADER_LEN: usize = 14;
const IPV4_MIN_HEADER_LEN: usize = 20;
const UDP_HEADER_LEN: usize = 8;
const MIN_UDP_FRAME_LEN: usize = ETH_HEADER_LEN + IPV4_MIN_HEADER_LEN + UDP_HEADER_LEN;
const RPC_PAYLOAD_BASE_OFF: usize = ETH_HEADER_LEN + IPV4_MIN_HEADER_LEN + UDP_HEADER_LEN;
const RPC_HEADER_SIZE: usize = 8 + 4 + 4;
const RPC_KEY_SIZE: usize = 10;
const RPC_ID_OFF_IN_FRAME: usize = RPC_PAYLOAD_BASE_OFF + 8;
const RPC_FANOUT_OFF_IN_FRAME: usize = RPC_PAYLOAD_BASE_OFF + RPC_HEADER_SIZE + RPC_KEY_SIZE;
const RPC_DEFAULT_DISPATCH_FANOUT: u32 = 1;
const RPC_MAX_DISPATCH_FANOUT: u32 = 9;
const RPC_RING_PACKET_MAX_BYTES: usize = 256;
pub const NUM_RPC_WORKER_THREADLET: usize = 6;
pub const NUM_RPC_WORKER_CORES: usize = 1;
pub const WORKER_CORE_ID: usize = 1;
const MAX_RPC_WORKER_CORES: usize = 64;
const MAX_TOTAL_RPC_WORKERS: usize = NUM_RPC_WORKER_THREADLET * MAX_RPC_WORKER_CORES;
// Currently, we define SoftIrqLine::NR_LINES=8 in  kernel/comps/softirq/src/lib.rs 
const ADDR_48BIT_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;
const RPC_STAGE: u64 = 7;
const RPC_SEND_STAGE: u64 = 87;

static ICENET_MASK_LOCK: SpinLock<(), LocalIrqDisabled> = SpinLock::new(());
static ICENET_TX_POOL: SpinLock<LinkedList<DmaStream>, LocalIrqDisabled> =
    SpinLock::new(LinkedList::new());
static POLL_RPC_WORKER_CORE_COUNT: AtomicUsize = AtomicUsize::new(0);
static RPC_DISPATCH_REQUEST_ID: AtomicU64 = AtomicU64::new(0);
#[repr(C, align(64))]
struct CacheLineU64 {
    value: AtomicU64,
    _pad: [u8; 64 - core::mem::size_of::<AtomicU64>()],
}

impl CacheLineU64 {
    const fn new(v: u64) -> Self {
        Self {
            value: AtomicU64::new(v),
            _pad: [0u8; 64 - core::mem::size_of::<AtomicU64>()],
        }
    }
}

#[derive(Clone, Copy)]
struct RpcRingPacketSlot {
    packet_len: u16,
    packet: [u8; RPC_RING_PACKET_MAX_BYTES],
}

const EMPTY_RPC_RING_PACKET_SLOT: RpcRingPacketSlot = RpcRingPacketSlot {
    packet_len: 0,
    packet: [0u8; RPC_RING_PACKET_MAX_BYTES],
};

struct RpcRingSlots {
    slots: UnsafeCell<[RpcRingPacketSlot; MAX_TOTAL_RPC_WORKERS]>,
}

#[allow(unsafe_code)]
unsafe impl Sync for RpcRingSlots {}

impl RpcRingSlots {
    const fn new() -> Self {
        Self {
            slots: UnsafeCell::new([EMPTY_RPC_RING_PACKET_SLOT; MAX_TOTAL_RPC_WORKERS]),
        }
    }

    #[allow(unsafe_code)]
    #[inline(always)]
    fn slot_mut(&self, worker_idx: usize) -> &mut RpcRingPacketSlot {
        unsafe {
            // SPSC discipline guarantees producer has exclusive write ownership of this slot
            // until it publishes the new head with Release ordering.
            let base = (*self.slots.get()).as_mut_ptr();
            &mut *base.add(worker_idx)
        }
    }

    #[allow(unsafe_code)]
    #[inline(always)]
    fn read_packet(&self, worker_idx: usize) -> &[u8] {
        unsafe {
            // Consumer reads only slots that are already published by producer via head(Acquire).
            let base = (*self.slots.get()).as_ptr();
            let slot = &*base.add(worker_idx);
            let packet_len = core::cmp::min(slot.packet_len as usize, RPC_RING_PACKET_MAX_BYTES);
            &slot.packet[..packet_len]
        }
    }
}

static RPC_RING_SLOTS: RpcRingSlots = RpcRingSlots::new();
static RPC_RING_HEAD: [CacheLineU64; MAX_TOTAL_RPC_WORKERS] =
    [const { CacheLineU64::new(0) }; MAX_TOTAL_RPC_WORKERS];
static RPC_RING_TAIL: [CacheLineU64; MAX_TOTAL_RPC_WORKERS] =
    [const { CacheLineU64::new(0) }; MAX_TOTAL_RPC_WORKERS];
static RPC_DISPATCH_NEXT_CORE: AtomicUsize = AtomicUsize::new(0);
static RPC_DISPATCH_NEXT_WORKER_PER_CORE: [AtomicUsize; MAX_RPC_WORKER_CORES] =
    [const { AtomicUsize::new(0) }; MAX_RPC_WORKER_CORES];

struct PollDoorbells {
    _pool: Arc<DmaPool>,
    _seg: DmaSegment,
    base_paddr: u64,
}

static ICENET_POLL_DOORBELLS: SpinOnce<PollDoorbells> = SpinOnce::new();

pub fn threadlet_polling_enabled() -> bool {
    use ostd::boot::{kcmdline::ModuleArg, kernel_cmdline};
    let kcmd = kernel_cmdline();

    if kcmd
        .get_initproc_argv()
        .iter()
        .any(|a| a.as_bytes() == b"threadlet_polling")
    {
        return true;
    }
    if kcmd.get_initproc_envp().iter().any(|e| {
        let b = e.as_bytes();
        b == b"threadlet_polling=1" || b == b"threadlet_polling=true"
    }) {
        return true;
    }
    false
}

pub fn threadlet_udp_waiter_enabled() -> bool {
    false
}


#[inline(always)]
fn compute_poll_rpc_worker_core_count() -> usize {
    ostd::cpu::num_cpus().saturating_sub(WORKER_CORE_ID)
}

pub fn init_poll_rpc_worker_topology() {
    if !threadlet_polling_enabled() {
        return;
    }

    let worker_core_count = compute_poll_rpc_worker_core_count();
    assert!(
        worker_core_count > 0,
        "threadlet_polling requires at least one AP worker core"
    );
    assert!(
        worker_core_count <= MAX_RPC_WORKER_CORES,
        "poll rpc worker core count {} exceed static capacity {}",
        worker_core_count,
        MAX_RPC_WORKER_CORES
    );
    POLL_RPC_WORKER_CORE_COUNT.store(worker_core_count, Ordering::Relaxed);
}

#[inline(always)]
pub fn poll_rpc_worker_core_count() -> usize {
    let cached = POLL_RPC_WORKER_CORE_COUNT.load(Ordering::Relaxed);
    if cached != 0 {
        return cached;
    }
    if !threadlet_polling_enabled() {
        return NUM_RPC_WORKER_CORES;
    }

    let worker_core_count = compute_poll_rpc_worker_core_count();
    assert!(
        worker_core_count > 0 && worker_core_count <= MAX_RPC_WORKER_CORES,
        "invalid poll rpc worker core count {} (capacity {})",
        worker_core_count,
        MAX_RPC_WORKER_CORES
    );
    POLL_RPC_WORKER_CORE_COUNT.store(worker_core_count, Ordering::Relaxed);
    worker_core_count
}

fn poll_doorbells_init() -> Result<&'static PollDoorbells, ostd::Error> {
    if let Some(v) = ICENET_POLL_DOORBELLS.get() {
        return Ok(v);
    }
    // Allocate a single page of coherent DMA memory for all per-queue polling doorbells.
    // Doorbell q is located at: base + q * 64 (one cacheline per queue).
    let pool = DmaPool::new(
        ostd::mm::PAGE_SIZE, // segment_size (one page, contiguous cachelines)
        1,  // init pages
        1,  // high watermark
        ostd::mm::DmaDirection::Bidirectional,
        true, // cache-coherent on Rocket
    );
    let seg = pool.alloc_segment()?;
    let base_paddr = seg.daddr() as u64;

    // Ensure the segment is large enough and clear the doorbell area.
    let need_bytes = NUM_CORES * DOORBELL_CACHELINE_BYTES;
    debug_assert!(seg.size() >= need_bytes);
    let zero = [0u8; DOORBELL_CACHELINE_BYTES * NUM_CORES];
    if let Ok(mut w) = seg.writer() {
        let _ = w
            .limit(need_bytes)
            .write(&mut VmReader::from(&zero as &[u8]));
    }

    Ok(ICENET_POLL_DOORBELLS.call_once(|| PollDoorbells {
        _pool: pool,
        _seg: seg,
        base_paddr,
    }))
}

fn configure_polling_mode(mmio: &IoMem) {
    let Ok(db) = poll_doorbells_init() else {
        ostd::early_println!("[icenet-os] WARN: polling doorbell alloc failed, polling disabled");
        return;
    };

    // Program per-queue polling doorbell addresses.
    for q in 0..NUM_CORES {
        let addr = db.base_paddr + (q as u64) * (DOORBELL_CACHELINE_BYTES as u64);
        let off = 0x110 + (q * 8);
        let _ = mmio.write_once::<u64>(off, &addr);
    }

    // Disable poll writer printf by default.
    let _ = mmio.write_once::<u32>(0x108, &0u32);

    // Enable polling for all queues.
    let mask: u32 = if NUM_CORES >= 32 {
        u32::MAX
    } else {
        (1u32 << NUM_CORES) - 1
    };
    let _ = mmio.write_once::<u32>(0x100, &mask);


    ostd::early_println!("[icenet-os] polling mode enabled mask=0x{:x}", mask);
}

pub fn polling_doorbell_paddr(q: usize) -> Option<u64> {
    let db = ICENET_POLL_DOORBELLS.get()?;
    if q >= NUM_CORES {
        return None;
    }
    Some(db.base_paddr + (q as u64) * (DOORBELL_CACHELINE_BYTES as u64))
}

#[inline(always)]
fn read_segment_bytes(seg: &DmaSegment, off: usize, dst: &mut [u8]) -> bool {
    let Some(end) = off.checked_add(dst.len()) else {
        return false;
    };
    if end > seg.size() {
        return false;
    }
    let Ok(mut reader) = seg.reader() else {
        return false;
    };
    let src = reader.skip(off).cursor();
    #[allow(unsafe_code)]
    unsafe {
        core::ptr::copy_nonoverlapping(src, dst.as_mut_ptr(), dst.len());
    }
    true
}

#[inline(always)]
fn store_be_u32(buf: &mut [u8], off: usize, value: u32) -> bool {
    let Some(end) = off.checked_add(4) else {
        return false;
    };
    let Some(slice) = buf.get_mut(off..end) else {
        return false;
    };
    slice.copy_from_slice(&value.to_be_bytes());
    true
}

#[inline(always)]
fn load_rpc_dispatch_fanout(seg: &DmaSegment, hdr_off: usize, pkt_bytes: usize) -> u32 {
    let frame_off = RPC_FANOUT_OFF_IN_FRAME;
    if frame_off + 4 > pkt_bytes {
        return RPC_DEFAULT_DISPATCH_FANOUT;
    }

    let mut raw = [0u8; 4];
    if !read_segment_bytes(seg, hdr_off + frame_off, &mut raw) {
        return RPC_DEFAULT_DISPATCH_FANOUT;
    }

    let fanout = u32::from_be_bytes(raw);
    fanout
}


#[inline(always)]
const fn global_worker_index(worker_core_idx: usize, worker_idx: usize) -> usize {
    worker_core_idx * NUM_RPC_WORKER_THREADLET + worker_idx
}

#[inline(always)]
fn normalize_worker_idx(worker_idx: usize) -> usize {
    worker_idx
}

#[inline(always)]
pub fn rpc_worker_try_process_one<F>(worker_idx: usize, f: F) -> bool
where
    F: FnOnce(&[u8]),
{
    let worker_idx = normalize_worker_idx(worker_idx);
    let tail = RPC_RING_TAIL[worker_idx].value.load(Ordering::Relaxed);
    let head = RPC_RING_HEAD[worker_idx].value.load(Ordering::Acquire);
    if head == tail {
        return false;
    }

    let packet = RPC_RING_SLOTS.read_packet(worker_idx);
    f(packet);
    true
}

#[inline(always)]
pub fn rpc_worker_finish_one(worker_idx: usize) {
    let worker_idx = normalize_worker_idx(worker_idx);
    let tail = RPC_RING_TAIL[worker_idx].value.load(Ordering::Relaxed);
    RPC_RING_TAIL[worker_idx]
        .value
        .store(tail.wrapping_add(1), Ordering::Release);
}

#[inline(always)]
pub fn rpc_worker_ring_empty(worker_idx: usize) -> bool {
    let worker_idx = normalize_worker_idx(worker_idx);
    let tail = RPC_RING_TAIL[worker_idx].value.load(Ordering::Relaxed);
    let head = RPC_RING_HEAD[worker_idx].value.load(Ordering::Acquire);
    head == tail
}

#[inline(always)]
pub fn rpc_worker_head_vaddr(worker_idx: usize) -> usize {
    let worker_idx = normalize_worker_idx(worker_idx);
    (&RPC_RING_HEAD[worker_idx].value as *const AtomicU64) as usize
}

#[inline(always)]
pub fn rpc_worker_head_load(worker_idx: usize) -> u64 {
    let worker_idx = normalize_worker_idx(worker_idx);
    RPC_RING_HEAD[worker_idx].value.load(Ordering::Acquire)
}

#[inline(always)]
fn detect_rx_header_offset(seg: &DmaSegment, pkt_len: usize) -> usize {
    if pkt_len < 16 {
        return 0;
    }
    let mut head = [0u8; 16];
    if seg.sync(0..16).is_err() {
        return 0;
    }
    let nread = if let Ok(mut reader) = seg.reader() {
        reader
            .limit(16)
            .read(&mut VmWriter::from(&mut head as &mut [u8]))
    } else {
        0
    };
    if nread < 16 {
        return 0;
    }

    let et12 = u16::from_be_bytes([head[12], head[13]]);
    let et14 = u16::from_be_bytes([head[14], head[15]]);
    if et12 == 0x0000 && (et14 == 0x0800 || et14 == 0x0806) {
        2
    } else {
        0
    }
}

pub struct PollingFastPath {
    q: usize,
    shared: Arc<IceNetShared>,
    rx_inflight: VecDeque<DmaSegment>,
    rx_recycle: VecDeque<DmaSegment>,
    hdr_off: usize,
    hdr_off_ready: bool,
}

impl PollingFastPath {
    #[inline(always)]
    fn refill_rx_queue(&mut self) {
        while let Some(seg) = self.rx_recycle.pop_front() {
            let space: u8 = self
                .shared
                .mmio
                .read_once(0xD0 + (self.q * 2))
                .unwrap_or(0);
            if space == 0 {
                self.rx_recycle.push_front(seg);
                break;
            }

            let daddr = seg.daddr() as u64;
            debug_assert!((daddr & !ADDR_48BIT_MASK) == 0, "RX daddr(0x{:x}) exceed 48 bit", daddr);
            debug_assert_eq!((daddr as usize) & 63, 0, "RX daddr must be 64B-aligned");
            mmio_wmb();
            let _ = self
                .shared
                .mmio
                .write_once::<u64>(0x30 + (self.q * 8), &daddr);
            self.rx_inflight.push_back(seg);
        }
    }

    #[inline(always)]
    fn resolve_hdr_off(&mut self, seg: &DmaSegment, pkt_len: usize) -> usize {
        if self.hdr_off_ready {
            return self.hdr_off;
        }
        let off = detect_rx_header_offset(seg, pkt_len);
        self.hdr_off = off;
        self.hdr_off_ready = true;
        if off == 2 {
            self.shared.tx_head_pad2.store(true, Ordering::Relaxed);
        }
        off
    }

    #[inline(always)]
    fn find_empty_worker_slot_rr(&self) -> (usize, u64, usize, usize) {
        let worker_core_count = poll_rpc_worker_core_count();
        loop {
            let start_core = RPC_DISPATCH_NEXT_CORE.load(Ordering::Relaxed) % worker_core_count;
            for core_probe in 0..worker_core_count {
                let worker_core_idx = (start_core + core_probe) % worker_core_count;
                let start_worker = RPC_DISPATCH_NEXT_WORKER_PER_CORE[worker_core_idx]
                    .load(Ordering::Relaxed)
                    % NUM_RPC_WORKER_THREADLET;

                for worker_probe in 0..NUM_RPC_WORKER_THREADLET {
                    let worker_idx = (start_worker + worker_probe) % NUM_RPC_WORKER_THREADLET;
                    let global_idx = global_worker_index(worker_core_idx, worker_idx);
                    let head_seq = RPC_RING_HEAD[global_idx].value.load(Ordering::Relaxed);
                    let tail_seq = RPC_RING_TAIL[global_idx].value.load(Ordering::Acquire);
                    // Single-slot queue: empty iff head == tail; full otherwise.
                    if head_seq == tail_seq {
                        return (global_idx, head_seq, worker_core_idx, worker_idx);
                    }
                }
            }
        }
    }

    #[inline(always)]
    fn dispatch_packet_to_worker(&mut self, seg: &DmaSegment, pkt_len: usize, hdr_off: usize) {
        let available = seg.size().saturating_sub(hdr_off);
        let pkt_bytes = core::cmp::min(pkt_len, available);
        if pkt_bytes < MIN_UDP_FRAME_LEN || pkt_bytes > RPC_RING_PACKET_MAX_BYTES {
            return;
        }
        threadlet::threadlet_syn_print(RPC_STAGE, 11);

        let dispatch_fanout = load_rpc_dispatch_fanout(seg, hdr_off, pkt_bytes);
        for _ in 0..dispatch_fanout {
            // When all worker slots are full, keep round-robin probing until one worker consumes.
            let (target_worker_global, target_head, target_core, target_worker) =
                self.find_empty_worker_slot_rr();
            threadlet::threadlet_syn_print(RPC_STAGE, 12);

            let slot = RPC_RING_SLOTS.slot_mut(target_worker_global);
            if !read_segment_bytes(seg, hdr_off, &mut slot.packet[..pkt_bytes]) {
                return;
            }
            let dispatched_id = RPC_DISPATCH_REQUEST_ID.fetch_add(1, Ordering::Relaxed) as u32;
            let _ = store_be_u32(&mut slot.packet[..pkt_bytes], RPC_ID_OFF_IN_FRAME, dispatched_id);
            slot.packet_len = pkt_bytes as u16;

            threadlet::threadlet_syn_print(RPC_SEND_STAGE, dispatched_id as u64);
            RPC_RING_HEAD[target_worker_global]
                .value
                .store(target_head.wrapping_add(1), Ordering::Release);
            threadlet::threadlet_syn_print(RPC_STAGE, 13);   

            let worker_core_count = poll_rpc_worker_core_count();
            RPC_DISPATCH_NEXT_CORE.store((target_core + 1) % worker_core_count, Ordering::Relaxed);
            RPC_DISPATCH_NEXT_WORKER_PER_CORE[target_core]
                .store((target_worker + 1) % NUM_RPC_WORKER_THREADLET, Ordering::Relaxed);
        }
    }
}

impl Drop for PollingFastPath {
    fn drop(&mut self) {
        let mut posted = self.shared.rx_posted[self.q].lock();
        while let Some(seg) = self.rx_inflight.pop_front() {
            posted.push_back(seg);
        }
        while let Some(seg) = self.rx_recycle.pop_front() {
            posted.push_back(seg);
        }
    }
}

pub fn polling_fast_path_init(q: usize) -> Option<PollingFastPath> {
    if q >= NUM_CORES {
        return None;
    }
    let shared = ICENET_SHARED.get()?.clone();
    let rx_inflight = {
        let mut posted = shared.rx_posted[q].lock();
        core::mem::take(&mut *posted)
    };

    let hdr_off_ready = shared.tx_head_pad2.load(Ordering::Relaxed);
    let hdr_off = if hdr_off_ready { 2 } else { 0 };
    Some(PollingFastPath {
        q,
        shared,
        rx_inflight,
        rx_recycle: VecDeque::new(),
        hdr_off,
        hdr_off_ready,
    })
}



pub fn polling_fast_path_process(ctx: &mut PollingFastPath) {
    let q = ctx.q;
    let comp_off = 0xB0 + (q * 2);
    let comp_cnt_off = 0xD0 + (q * 2) + 1;

    loop {
        let mut to_deq: usize = ctx.shared.mmio.read_once::<u8>(comp_cnt_off).unwrap_or(0) as usize;
        if to_deq == 0 {
            ctx.refill_rx_queue();
            break;
        }

        while to_deq > 0 {
            let len: u16 = ctx.shared.mmio.read_once(comp_off).unwrap_or(0);
            if len == 0 {
                break;
            }
            mmio_rmb();

            let Some(seg) = ctx.rx_inflight.pop_front() else {
                ostd::early_println!("[icenet-poll-fast] WARN: RX{} completion with empty inflight", q);
                break;
            };

            threadlet::threadlet_syn_print(RPC_STAGE,10);

            let pkt_len = len as usize;
            let hdr_off = ctx.resolve_hdr_off(&seg, pkt_len);
            ctx.dispatch_packet_to_worker(&seg, pkt_len, hdr_off);

            ctx.rx_recycle.push_back(seg);
            ctx.refill_rx_queue();
            to_deq -= 1;
        }

        let comp_cnt: u8 = ctx.shared.mmio.read_once(comp_cnt_off).unwrap_or(0);
        if comp_cnt == 0 {
            ctx.refill_rx_queue();
            break;
        }
    }
}

pub fn polling_minimal_process_queue(q: usize) {
    if let Some(mut ctx) = polling_fast_path_init(q) {
        polling_fast_path_process(&mut ctx);
    }
}

/// Runs the icenet RX/TX bottom-half for a given queue `q`.
///
/// This function implements the core NAPI-like polling logic used by both
/// the generic softirq handler and the dedicated bottom-half threadlet.
pub fn icenet_softirq_handler_for_queue(q: usize) {
    if let Some(shared) = ICENET_SHARED.get() {
        let mmio = &shared.mmio;
        if cfg!(irqdebug) {
            ostd::early_println!("[icenet-os] softirq enter q={}", q);
        }
        
        // start polling
        loop {
            let rx_cnt = shared.poll_rx_queue(q);
            let tx_cnt = shared.reclaim_tx();
            if cfg!(irqdebug) {     
                ostd::early_println!(
                    "[icenet-os] softirq(q={}) processed rx={} txc={}",
                    q, rx_cnt, tx_cnt
                );
            }
            ICENET_ACTIVE_Q.store(q);
            threadlet::threadlet_syn_print(5, 131);
            crate::handle_recv_irq(DEVICE_NAME);
            threadlet::threadlet_syn_print(5, 132);
            let comp_cnt_off = 0xD0 + (q * 2) + 1;
            let comp_cnt: u8 = mmio.read_once(comp_cnt_off).unwrap_or(0);
            let sw_empty = shared.rx_completed_is_empty(q);

            if comp_cnt == 0 && sw_empty {
                let before = mmio.read_once::<u32>(0xF0).unwrap_or(0);
                update_int_mask(mmio, |cur| (cur | (1u32 << (1 + q as u32))) & !0x1u32);
                let after = mmio.read_once::<u32>(0xF0).unwrap_or(before);
                if cfg!(irqdebug) {
                    ostd::early_println!(
                        "[icenet-os] softirq(q={}) unmask RX & keep TX off 0x{:x}->0x{:x}",
                        q, before, after
                    );
                }
                threadlet::threadlet_syn_print(5, 133);
                break;
            }
        }
    }
}

fn update_int_mask(mmio: &IoMem, f: impl FnOnce(u32) -> u32) {
    let _g = ICENET_MASK_LOCK.lock();
    let cur: u32 = mmio.read_once(0xF0).unwrap_or(0);
    let newv = f(cur);
    let _ = mmio.write_once::<u32>(0xF0, &newv);
}

#[cfg(target_arch = "riscv64")]
fn read_mac_from_mmio(mmio: &IoMem) -> [u8; 6] {
    let mut mac = [0u8; 6];
    for i in 0..6 {
        mac[i] = mmio.read_once(0x18 + i).unwrap_or(0u8);
    }
    mac
}

pub fn register_if_present() {
    #[cfg(target_arch = "riscv64")]
    {
        init_poll_rpc_worker_topology();
        assert!(
            (NIC_SOFTIRQ_BASE as usize + NUM_CORES) <= aster_softirq::SoftIrqLine::nr_lines() as usize,
            "icenet: NIC_SOFTIRQ_BASE({}) + NUM_CORES({}) exceed softirq num({})",
            NIC_SOFTIRQ_BASE,
            NUM_CORES,
            aster_softirq::SoftIrqLine::nr_lines()
        );

        if let Some(mmio) = ostd::arch::riscv::device::icenet::get_mmio() {
            let mac = read_mac_from_mmio(&mmio);
            // mask all interrupts initially
            let _ = mmio.write_once::<u32>(0xF0, &0x0u32);
            if cfg!(netdebug) { ostd::early_println!("[icenet-os] INT_MASK=0x0 (all masked)"); }

            let shared = Arc::new(IceNetShared::new(mmio.clone()));
            shared.init_rings();

            let dev_arc: Arc<SpinLock<IceNetDevice, LocalIrqDisabled>> =
                Arc::new(SpinLock::new(IceNetDevice::new(mac, shared.clone())));


            crate::register_device(DEVICE_NAME.into(), dev_arc.clone());
            ICENET_DEV.call_once(|| dev_arc.clone());
            ICENET_SHARED.call_once(|| shared.clone());

            // Optional: threadlet polling mode uses per-queue doorbell DMA writes instead of interrupts.
            if threadlet_polling_enabled() {
                // Keep INT_MASK at 0 in polling mode.
                let _ = mmio.write_once::<u32>(0xF0, &0x0u32);
                configure_polling_mode(&mmio);
            }

            for q in 0..NUM_CORES {
                use ostd::arch::boot::smp::intr_threadlet_present;
                if intr_threadlet_present()
                {
                    SoftIrqLine::get(NIC_SOFTIRQ_BASE + q as u8).enable(move || {
                        ostd::early_println!("[icenet-os] ****warning*** this should not trigger for q={}", q);
                    });
                }
                else
                {
                    SoftIrqLine::get(NIC_SOFTIRQ_BASE + q as u8).enable(move || {
                    if let Some(shared) = ICENET_SHARED.get() {
                        let mmio = &shared.mmio;
                        if cfg!(irqdebug) {
                            ostd::early_println!("[icenet-os] softirq enter q={}", q);
                        }
                        // start polling
                        loop {
                            
                            let rx_cnt = shared.poll_rx_queue(q);
                            let tx_cnt = shared.reclaim_tx();
                            if cfg!(irqdebug) {
                                ostd::early_println!(
                                    "[icenet-os] softirq(q={}) processed rx={} txc={}",
                                    q, rx_cnt, tx_cnt
                                );
                            }
                            // call protocol stack if we got any RX
                            ICENET_ACTIVE_Q.store(q);
                            crate::handle_recv_irq(DEVICE_NAME);

                            let comp_cnt_off = 0xD0 + (q * 2) + 1;
                            let comp_cnt: u8 = mmio.read_once(comp_cnt_off).unwrap_or(0);
                            let sw_empty = shared.rx_completed_is_empty(q);

                            if comp_cnt == 0 && sw_empty {
                                let before = mmio.read_once::<u32>(0xF0).unwrap_or(0);
                                update_int_mask(mmio, |cur| (cur | (1u32 << (1 + q as u32))) & !0x1u32);
                                let after = mmio.read_once::<u32>(0xF0).unwrap_or(before);
                                if cfg!(irqdebug) { ostd::early_println!("[icenet-os] softirq(q={}) unmask RX & keep TX off 0x{:x}->0x{:x}", q, before, after); }
                                break;
                            }
                        }
                    }
                });
                }
                
            }


	            ostd::arch::riscv::device::icenet::set_irq_hook(|idx| {
	                ostd::arch::riscv::threadlet::threadlet_syn_print(5, 1111);
	                // idx==0 TX；idx>=1 RX_i
	                if idx == 0 { return; }
	                let q = idx - 1; // RX number
	                if let Some(mmio) = ostd::arch::riscv::device::icenet::get_mmio() {
	                    let _before = mmio.read_once::<u32>(0xF0).unwrap_or(0);
	                    update_int_mask(&mmio, |cur| cur & !(1u32 << (1 + q as u32)));
	                    let _after = mmio.read_once::<u32>(0xF0).unwrap_or(_before);
	                    let q_u8 = q as u8;
	                    if !wake_icenet_bh_threadlet(q_u8) {
	                        // Fallback to the legacy softirq line path if the dedicated BH threadlet is not available
	                        SoftIrqLine::get(NIC_SOFTIRQ_BASE + q_u8).raise();
	                    }
	                }
	                ostd::arch::riscv::threadlet::threadlet_syn_print(5, 1111);
	            });

            // we disable TX interrupts permanently, RX interrupts will be enabled
            // when the first recv callback is registered.
            if let Some(mm) = ostd::arch::riscv::device::icenet::get_mmio() {
                update_int_mask(&mm, |cur| cur & !0x1);
            }
            if cfg!(netdebug) {
                let mac_val = read_mac_from_mmio(&mmio);
                ostd::early_println!(
                    "[icenet-os] registered '{}' mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}; INT_MASK=0x0 (TX disabled)",
                    DEVICE_NAME, mac_val[0], mac_val[1], mac_val[2], mac_val[3], mac_val[4], mac_val[5]
                );
            }
        }
    }
}

/// Enables RX interrupts for all queues.
/// This should be called only when the network stack is ready to process packets.
pub fn enable_rx_interrupts() {
    if threadlet_polling_enabled() {
        ostd::early_println!("[icenet-os] polling mode active: keep RX interrupts masked");
        return;
    }
    if let Some(shared) = ICENET_SHARED.get() {
        let mmio = &shared.mmio;
        // unmask RX interrupts for all queues, keep TX masked
        update_int_mask(mmio, |cur| (cur | 0x6) & !0x1);
        if cfg!(irqdebug) {
            let after = mmio.read_once::<u32>(0xF0).unwrap_or(0);
            ostd::early_println!("[icenet-os] RX interrupts enabled, INT_MASK=0x{:x}", after);
        }
    }
}

#[derive(Debug)]
struct IceNetDevice {
    mac: EthernetAddr,
    shared: Arc<IceNetShared>,
}

impl IceNetDevice {
    fn new(mac: [u8;6], shared: Arc<IceNetShared>) -> Self {
        Self {
            mac: EthernetAddr(mac),
            shared,
        }
    }
}


#[derive(Debug)]
struct IceNetShared {
    mmio: IoMem,
    rx_posted: [SpinLock<VecDeque<DmaSegment>, LocalIrqDisabled>; NUM_CORES],
    rx_completed: [SpinLock<VecDeque<RxBuffer>, LocalIrqDisabled>; NUM_CORES],
    rx_completed_count: [AtomicUsize; NUM_CORES],
    tx_pending: SpinLock<VecDeque<TxBuffer>, LocalIrqDisabled>,
    /// Whether TX path should prepend 2-byte pad for FPGA alignment.
    /// Set to true once RX path observes hdr_off=2 from hardware.
    tx_head_pad2: AtomicBool,
}

impl IceNetShared {
    fn new(mmio: IoMem) -> Self {
        Self {
            mmio,
            rx_posted: core::array::from_fn(|_| SpinLock::new(VecDeque::new())),
            rx_completed: core::array::from_fn(|_| SpinLock::new(VecDeque::new())),
            rx_completed_count: core::array::from_fn(|_| AtomicUsize::new(0)),
            tx_pending: SpinLock::new(VecDeque::new()),
            tx_head_pad2: AtomicBool::new(false),
        }
    }

    fn init_rings(&self) {
        let rx_pool = RX_BUFFER_POOL.get().unwrap();
        for q in 0..NUM_CORES {
            // Debug: observe hardware space before posting
            let space_before: u8 = self.mmio.read_once(0xD0 + (q * 2)).unwrap_or(0);
            let mut posted = self.rx_posted[q].lock();
            let mut posted_cnt = 0usize;
            // Try to fill the hardware RX address queue respecting its space counter.
            while posted_cnt < RX_RING_DEPTH {
                let space_now: u8 = self.mmio.read_once(0xD0 + (q * 2)).unwrap_or(0);
                if space_now == 0 { break; }
                let seg = rx_pool.alloc_segment().unwrap();
                // Pre-fill a marker pattern in the first 64 bytes to help diagnose DMA writes.
                if let Ok(mut w) = seg.writer() {
                    let mut pat = [0u8; 64];
                    for b in &mut pat { *b = 0xCC; }
                    let _ = w.limit(64).write(&mut VmReader::from(&pat as &[u8]));
                }
                let daddr = seg.daddr();
                debug_assert!(
                    ((daddr as u64) & !ADDR_48BIT_MASK) == 0,
                    "RX posted daddr(0x{:x}) exceed 48 bit",
                    daddr
                );
                debug_assert_eq!((daddr as usize) & 63, 0, "RX posted daddr must be 64B-aligned");
                posted.push_back(seg);
                // Ensure the descriptor write to MMIO is not reordered before prior stores
                mmio_wmb();
                let off = 0x30 + (q * 8) as usize;
                let _ = self.mmio.write_once::<u64>(off, &(daddr as u64));
                posted_cnt += 1;
                if cfg!(irqdebug) { ostd::early_println!("[icenet-os] RX{} post daddr=0x{:x} space->{}", q, daddr, space_now.saturating_sub(1)); }
            }
            let space_after: u8 = self.mmio.read_once(0xD0 + (q * 2)).unwrap_or(0);
            ostd::early_println!(
                "[icenet-os] init_rings q={} space_before={} posted={} space_after={}",
                q, space_before, posted_cnt, space_after
            );
        }
        if cfg!(netdebug) { ostd::early_println!("[icenet-os] rings initialized per-queue"); }
    }

    fn poll_rx_queue(&self, q: usize) -> usize {
        let comp_off = 0xB0 + (q * 2) as usize;
        let mut done = 0usize;
        let mut posted = self.rx_posted[q].lock();

        // Read completion count snapshot first; limit dequeue attempts to this value.
        let comp_cnt_off = 0xD0 + (q * 2) as usize + 1;
        let mut to_deq: usize = self.mmio.read_once::<u8>(comp_cnt_off).unwrap_or(0) as usize;
        if to_deq == 0 {
            return 0;
        }
        if cfg!(irqdebug) {
            ostd::early_println!("[icenet-os] RX{} plan to deq {} comps (by count)", q, to_deq);
        }
        while to_deq > 0 {
            let len: u16 = self.mmio.read_once(comp_off).unwrap_or(0);
            if cfg!(irqdebug) {
                ostd::early_println!("[icenet-os] RX{} comp_deq got len={}", q, len);
            }
            if len == 0 {
                // Hardware said there were completions but we saw 0; stop to avoid blocking.
                break;
            }
            // Order: make DMA writes visible to CPU before subsequent loads of the RX buffer.
            mmio_rmb();
            if let Some(seg) = posted.pop_front() {
                // Detect per-packet Ethernet header alignment.
                // On FPGA icenet, ETH_HEAD_BYTES=16 and NET_IP_ALIGN=2, which makes
                // the software see a 2-byte shift compared to the standard 14-byte
                // Ethernet header. Here we peek the first 16 bytes and choose
                // header_len = 0 (QEMU/standard) or 2 (FPGA-aligned) accordingly.
                let pkt_len = len as usize;
                let mut hdr_off: usize = 0;
                if pkt_len >= 16 {
                    // Ensure device DMA writes are visible before reading from memory.
                    // mmio_rmb() has already been issued above for ordering, and
                    // our DMA mappings are cache-coherent; we still best-effort
                    // read the head now.
                    let mut head = [0u8; 16];
                    // Sync the first 16 bytes into cache and copy into head
                    if seg.sync(0..16).is_ok() {
                        if let Ok(mut r) = seg.reader() {
                            let _ = r.limit(16).read(&mut VmWriter::from(&mut head as &mut [u8]));
                            let et12 = u16::from_be_bytes([head[12], head[13]]);
                            // Common ethertypes we care first: IPv4(0x0800), ARP(0x0806)
                            let et14 = u16::from_be_bytes([head[14], head[15]]);
                            if et12 == 0x0000 && (et14 == 0x0800 || et14 == 0x0806) {
                                hdr_off = 2;
                                // Remember that hardware places a 2-byte pad before Ethernet header.
                                // TX should mirror this to avoid left-shifted frames on the wire.
                                self.tx_head_pad2.store(true, Ordering::Relaxed);
                            } else {
                                hdr_off = 0;
                            }
                            if cfg!(irqdebug) {
                                ostd::early_println!(
                                    "[icenet-os] RX{} align-detect: daddr=0x{:x} et12=0x{:04x} et14=0x{:04x} hdr_off={}",
                                    q, seg.daddr(), et12, et14, hdr_off
                                );
                                // If the head looks all zeros or marker 0xCC, warn once.
                                // let all_zero = head.iter().all(|&b| b == 0);
                                // let all_cc = head.iter().all(|&b| b == 0xCC);
                                // if all_zero || all_cc {
                                //     ostd::early_println!(
                                //         "[icenet-os] WARN: RX{} head suspicious (zero/marker) daddr=0x{:x} len={}",
                                //         q, seg.daddr(), pkt_len
                                //     );
                                // }
                            }
                        }
                    }
                }

                let mut rx = RxBuffer::from_segment(seg, hdr_off);
                rx.set_packet_len(pkt_len);
                {
                    let mut completed = self.rx_completed[q].lock();
                    completed.push_back(rx);
                }
                // Bump the per-queue completed RX count for fast can_receive() path.
                self.rx_completed_count[q].fetch_add(1, Ordering::Relaxed);
                // Immediately re-post a fresh buffer; honor hardware space to avoid overflow
                let rx_pool = RX_BUFFER_POOL.get().unwrap();
                let new_seg = rx_pool.alloc_segment().unwrap();
                let daddr = new_seg.daddr();
                posted.push_back(new_seg);
                // Only post if there is space; otherwise defer (the lock holds the buffer)
                let space_now: u8 = self.mmio.read_once(0xD0 + (q * 2)).unwrap_or(0);
                if space_now != 0 {
                    // Pre-fill marker for diagnostics
                    if let Ok(mut w) = posted.back().unwrap().writer() {
                        let mut pat = [0u8; 64];
                        for b in &mut pat { *b = 0xCC; }
                        let _ = w.limit(64).write(&mut VmReader::from(&pat as &[u8]));
                    }
                    mmio_wmb();
                    let off = 0x30 + (q * 8) as usize;
                    let _ = self.mmio.write_once::<u64>(off, &(daddr as u64));
                    if cfg!(irqdebug) { ostd::early_println!("[icenet-os] RX{} repost daddr=0x{:x} space->{}", q, daddr, space_now.saturating_sub(1)); }
                } else {
                    if cfg!(irqdebug) { ostd::early_println!("[icenet-os] RX{} defer repost (no space) daddr=0x{:x}", q, daddr); }
                }
            } else {
                ostd::early_println!("[icenet-os] WARN: RX{}_comp with empty posted ring", q);
            }
            done += 1;
            to_deq -= 1;
        }
        done
    }

    #[doc = " 回收所有 TX 完成项（读取 0x08 直到返回 0）。返回回收的数量。"]
    fn reclaim_tx(&self) -> usize {
        // Read send completion count snapshot (0x10 high byte), then pop that many from 0x08.
        let mut txc = 0usize;
        let mut txq = self.tx_pending.lock();
        let cnt_off = 0x10 + 1; // [15:8] sendCompCount
        let mut to_deq: usize = self.mmio.read_once::<u8>(cnt_off).unwrap_or(0) as usize;
        if to_deq == 0 {
            return 0;
        }
        if cfg!(irqdebug) {
            ostd::early_println!("[icenet-os] TX plan to reclaim {} comps (by count)", to_deq);
        }
        while to_deq > 0 {
            let v = self.mmio.read_once::<u32>(0x08).unwrap_or(0);
            if v == 0 { break; }
            let _ = txq.pop_front();
            txc += 1;
            to_deq -= 1;
        }
        if cfg!(irqdebug) && txc > 0 { ostd::early_println!("[icenet-os] TX reclaimed {} (tx-lock)", txc); }
        txc
    }

    fn rx_completed_is_empty(&self, q: usize) -> bool {
        self.rx_completed[q].lock().is_empty()
    }

    fn rx_completed_len(&self, q: usize) -> usize {
        self.rx_completed[q].lock().len()
    }

    fn tx_pending_len(&self) -> usize {
        self.tx_pending.lock().len()
    }
}

impl AnyNetworkDevice for IceNetDevice {
    fn mac_addr(&self) -> EthernetAddr { self.mac }

    fn capabilities(&self) -> aster_bigtcp::device::DeviceCapabilities {
        let mut caps = aster_bigtcp::device::DeviceCapabilities::default();
        caps.max_transmission_unit = 1500;
        caps.medium = aster_bigtcp::device::Medium::Ethernet;
        caps
    }

    fn can_receive(&self) -> bool {
        let q = ICENET_ACTIVE_Q.load();
        if q < NUM_CORES {
            // Fast-path via atomic counter to avoid taking locks in can_receive.
            self.shared.rx_completed_count[q].load(Ordering::Relaxed) != 0
        } else {
            false
        }
    }

    fn can_send(&self) -> bool {


        let space: u8 = self.shared.mmio.read_once(0x10).unwrap_or(0);
        space != 0
    }

    fn receive(&mut self) -> Result<RxBuffer, VirtioNetError> {
        let q = ICENET_ACTIVE_Q.load();
        if q < NUM_CORES {
            let mut completed = self.shared.rx_completed[q].lock();
            let before = completed.len();
            if let Some(rx) = completed.pop_front() {
                let after = completed.len();
                // Decrement the per-queue completed counter.
                self.shared.rx_completed_count[q].fetch_sub(1, Ordering::Relaxed);
                return Ok(rx);
            } 
        } else {
            ostd::early_println!(
                "[icenet-os] receive() NotReady active_q={} (invalid)",
                q
            );
        }
        Err(VirtioNetError::NotReady)
    }


















// Notes:






    fn send(&mut self, packet: &[u8]) -> Result<(), VirtioNetError> {


        let pad2 = self.shared.tx_head_pad2.load(Ordering::Relaxed);
        let header_pad: [u8; 2] = [0u8; 2];
        let txb = if pad2 {
            TxBuffer::new_raw(&header_pad, packet, &ICENET_TX_POOL)
        } else {
            TxBuffer::new_raw(&[], packet, &ICENET_TX_POOL)
        };
        let daddr = txb.daddr() as u64;
        debug_assert!(
            (daddr & !ADDR_48BIT_MASK) == 0,
            "TX daddr(0x{:x}) exceed 48 bit",
            daddr
        );
        debug_assert_eq!((daddr as usize) & 63, 0, "TX daddr must be 64B-aligned");
        let len = (packet.len() + if pad2 { 2 } else { 0 }) as u64;
        

        let desc = ((0u64) << 63) | ((len & 0x7fff) << 48) | (daddr & ADDR_48BIT_MASK);


        {
            let mut txq = self.shared.tx_pending.lock();

            let space2: u8 = self.shared.mmio.read_once(0x10).unwrap_or(0);
            if space2 == 0 {
                if cfg!(irqdebug) { ostd::early_println!("[icenet-os] TX no space (retry in lock)"); }
                return Err(VirtioNetError::Busy);
            }
            // Ensure TX buffer writes reach memory before the doorbell MMIO write.
            mmio_wmb();
            let _ = self.shared.mmio.write_once::<u64>(0x00, &desc);
            txq.push_back(txb);
            if cfg!(irqdebug) {
                ostd::early_println!(
                    "[icenet-os] TX submit len={} daddr=0x{:x} pad2={}",
                    len, daddr, pad2
                );
            }
        }
        Ok(())
    }

    fn free_processed_tx_buffers(&mut self) {}

    fn notify_poll_end(&mut self) {}
}

static ICENET_DEV: SpinOnce<Arc<SpinLock<IceNetDevice, LocalIrqDisabled>>> = SpinOnce::new();
static ICENET_SHARED: SpinOnce<Arc<IceNetShared>> = SpinOnce::new();
static ICENET_BH_THREADLET_HART: SpinOnce<u32> = SpinOnce::new();

/// Records the hart id of the dedicated icenet bottom-half threadlet.
///
/// This is used by the icenet hard interrupt hook to directly wake up the bottom-half threadlet
/// without raising a softirq line.
pub fn set_icenet_bh_threadlet_hart(hart_id: u32) {
    ICENET_BH_THREADLET_HART.call_once(|| hart_id);
}

#[cfg(target_arch = "riscv64")]
#[inline(always)]
fn wake_icenet_bh_threadlet(queue: u8) -> bool {
    let Some(hart) = ICENET_BH_THREADLET_HART.get() else {
        return false;
    };

    use aster_softirq::softirq_id::ICENET_SOFTIRQ_PRIO;
    use ostd::arch::riscv::threadlet;
    
    threadlet::threadlet_set_a0_quiet(*hart, queue as u64);
    threadlet::threadlet_wakeup(*hart);
    true
}

#[cfg(not(target_arch = "riscv64"))]
#[inline(always)]
fn wake_icenet_bh_threadlet(_queue: u8) -> bool {
    false
}


cpu_local_cell! {
    static ICENET_ACTIVE_Q: usize = usize::MAX;
}

/// Fast path to check if the active RX queue has available packets without taking locks.
#[cfg(target_arch = "riscv64")]
pub fn active_queue_has_data_fast() -> bool {
    if let Some(shared) = ICENET_SHARED.get() {
        let q = ICENET_ACTIVE_Q.load();
        if q < NUM_CORES {
            return shared.rx_completed_count[q].load(Ordering::Relaxed) != 0;
        }
    }
    false
}

/// Try to pop one completed RX buffer for the active queue without holding the
/// outer global device lock. This uses per-queue locks internally.
#[cfg(target_arch = "riscv64")]
pub fn try_receive_one() -> Option<RxBuffer> {
    if let Some(shared) = ICENET_SHARED.get() {
        let q = ICENET_ACTIVE_Q.load();
        if q < NUM_CORES {
            let mut completed = shared.rx_completed[q].lock();
            if let Some(rx) = completed.pop_front() {
                // Decrement fast-path counter to keep `active_queue_has_data_fast()` consistent.
                shared.rx_completed_count[q].fetch_sub(1, Ordering::Relaxed);
                return Some(rx);
            }
        }
    }
    None
}

/// Try to transmit one packet without holding the outer global device lock.
/// Uses the internal TX lock only; returns Busy/NotReady if congested.
#[cfg(target_arch = "riscv64")]
pub fn try_send_packet(packet: &[u8]) -> Result<(), VirtioNetError> {
    let Some(shared) = ICENET_SHARED.get() else { return Err(VirtioNetError::NotReady); };

    // Mirror `IceNetDevice::send` logic, but operate on shared state directly.
    let pad2 = shared.tx_head_pad2.load(Ordering::Relaxed);
    let header_pad: [u8; 2] = [0u8; 2];
    let txb = if pad2 {
        TxBuffer::new_raw(&header_pad, packet, &ICENET_TX_POOL)
    } else {
        TxBuffer::new_raw(&[], packet, &ICENET_TX_POOL)
    };
    let daddr = txb.daddr() as u64;
    debug_assert!((daddr & !ADDR_48BIT_MASK) == 0, "TX daddr(0x{:x}) exceed 48 bit", daddr);
    debug_assert_eq!((daddr as usize) & 63, 0, "TX daddr must be 64B-aligned");
    let len = (packet.len() + if pad2 { 2 } else { 0 }) as u64;

    // 64-bit descriptor: partial=0, len[62:48], addr[47:0]
    let desc = ((0u64) << 63) | ((len & 0x7fff) << 48) | (daddr & ADDR_48BIT_MASK);
    {
        let mut txq = shared.tx_pending.lock();
        // Check space again under lock
        let space2: u8 = shared.mmio.read_once(0x10).unwrap_or(0);
        if space2 == 0 { return Err(VirtioNetError::Busy); }
        mmio_wmb();
        let _ = shared.mmio.write_once::<u64>(0x00, &desc);
        txq.push_back(txb);
    }
    Ok(())
}

/// Send a gratuitous ARP from the icenet interface to announce our IP/MAC.
#[cfg(target_arch = "riscv64")]
pub fn send_gratuitous_arp() {
    use alloc::vec::Vec;

    // Resolve device and MAC
    let Some(dev) = ICENET_DEV.get() else { return; };
    let mac = dev.lock().mac_addr().0;

    // Determine our IPv4 address: use the same defaults and kcmdline overrides
    // as kernel/src/net/iface/init.rs::new_icenet()
    let mut addr: [u8; 4] = [172, 16, 0, 2];
    {
        use ostd::boot::{kcmdline::ModuleArg, kernel_cmdline};
        fn parse_ipv4(s: &str) -> Option<[u8; 4]> {
            let mut out = [0u8; 4];
            let segs: alloc::vec::Vec<&str> = s.split('.').collect();
            if segs.len() != 4 { return None; }
            for (i, p) in segs.iter().enumerate() { out[i] = p.parse::<u8>().ok()?; }
            Some(out)
        }
        fn parse_cidr(s: &str) -> Option<([u8;4], u8)> {
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
                if let ModuleArg::KeyVal(name, value) = a {
                    if name.as_bytes() == b"addr" {
                        if let Ok(v) = core::str::from_utf8(value.as_bytes()) {
                            if let Some((ip, _)) = parse_cidr(v) { addr = ip; }
                        }
                    }
                }
            }
        }
    }

    // Build ARP request (gratuitous): who-has our-ip tell our-ip (broadcast)
    // Ethernet header (14 bytes)
    let mut frame: Vec<u8> = Vec::with_capacity(60);
    // dst = broadcast
    frame.extend_from_slice(&[0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);
    frame.extend_from_slice(&mac);
    frame.push(0x08); frame.push(0x06);
    frame.push(0x00); frame.push(0x01);
    frame.push(0x08); frame.push(0x00);
    frame.push(6); frame.push(4);
    frame.push(0x00); frame.push(0x01);
    frame.extend_from_slice(&mac);
    frame.extend_from_slice(&addr);
    frame.extend_from_slice(&[0,0,0,0,0,0]);
    frame.extend_from_slice(&addr);

    // Pad to 60 bytes (no FCS in software)
    while frame.len() < 60 { frame.push(0); }

    if cfg!(netdebug) {
        ostd::early_println!(
            "[icenet-os] send_gratuitous_arp mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} ip={}.{}.{}.{} len={}",
            mac[0], mac[1], mac[2], mac[3], mac[4], mac[5], addr[0], addr[1], addr[2], addr[3], frame.len()
        );
    }

    // Send via device
    if let Some(dev) = ICENET_DEV.get() {
        let mut d = dev.lock();
        let _ = d.send(&frame);
    }
}
