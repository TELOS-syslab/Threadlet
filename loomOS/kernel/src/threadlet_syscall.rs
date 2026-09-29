// SPDX-License-Identifier: MPL-2.0

use super::{kernel_vaddr_to_paddr, CacheLineAtomicU64};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU64, Ordering};
use ostd::boot::kernel_cmdline;
use ostd::mm::{FrameAllocOptions, Segment, PAGE_SIZE};
use spin::Once as SpinOnce;

pub(crate) const MAX_SYSCALL_RING_SLOTS: usize = 128;
pub(crate) const SYSCALL_SHARED_BUF_BYTES: usize = 16 * 1024;
pub(crate) const SYSCALL_SLOT_DATA_BYTES: usize = SYSCALL_SHARED_BUF_BYTES / MAX_SYSCALL_RING_SLOTS;
pub(crate) const SYSCALL_REQ_ID_SENDTO: u32 = 1;
pub(crate) const SYSCALL_TEST_SEND_COUNT: usize = 10;
pub(crate) const SYSCALL_TEST_DST_IP: [u8; 4] = [172, 16, 0, 1];
pub(crate) const SYSCALL_TEST_DST_PORT: u16 = 11777;
pub(crate) const SYSCALL_DELEG_HANDLER_PRIO: u32 = 30;
pub(crate) const SYSCALL_DELEG_PRODUCER_PRIO: u32 = 10;
pub(crate) const SYSCALL_DELEG_READY_WAIT_SPINS: usize = 50_000_000;
pub(crate) const SYSCALL_STAGE: u64 = 30;
pub(crate) const SYSCALL_COMPLETION_PENDING: i64 = 0;
pub(crate) const SYSCALL_COMPLETION_OK: i64 = 1;
pub(crate) const SYSCALL_COMPLETION_ERR: i64 = -1;

#[derive(Clone, Copy)]
#[repr(C)]
pub(crate) struct SyscallDelegationReq {
    pub(crate) id: u32,
    pub(crate) seq: u32,
    pub(crate) buf_off: u32,
    pub(crate) buf_len: u32,
    pub(crate) dst_ipv4_be: u32,
    pub(crate) dst_port_be: u16,
    pub(crate) reserved: u16,
}

impl SyscallDelegationReq {
    const fn empty() -> Self {
        Self {
            id: 0,
            seq: 0,
            buf_off: 0,
            buf_len: 0,
            dst_ipv4_be: 0,
            dst_port_be: 0,
            reserved: 0,
        }
    }
}

#[repr(C)]
pub(crate) struct SyscallDelegationSlot {
    req: UnsafeCell<SyscallDelegationReq>,
    completion: CacheLineAtomicU64,
}

impl SyscallDelegationSlot {
    fn new() -> Self {
        Self {
            req: UnsafeCell::new(SyscallDelegationReq::empty()),
            completion: CacheLineAtomicU64::new(SYSCALL_COMPLETION_PENDING as u64),
        }
    }
}

unsafe impl Sync for SyscallDelegationSlot {}

const _: [(); 64] = [(); core::mem::size_of::<CacheLineAtomicU64>()];
const _: [(); 64] = [(); core::mem::align_of::<CacheLineAtomicU64>()];
const _: [(); 0] = [(); core::mem::size_of::<SyscallDelegationSlot>() % 64];

pub(crate) struct SyscallDelegationSlots {
    slots: [SyscallDelegationSlot; MAX_SYSCALL_RING_SLOTS],
}

impl SyscallDelegationSlots {
    fn new() -> Self {
        Self {
            slots: core::array::from_fn(|_| SyscallDelegationSlot::new()),
        }
    }

    #[inline(always)]
    fn slot_write_sendto(
        &self,
        idx: usize,
        seq: u32,
        buf_off: u32,
        buf_len: u32,
        dst_ipv4_be: u32,
        dst_port_be: u16,
    ) {
        #[allow(unsafe_code)]
        unsafe {
            let req = &mut *self.slots[idx].req.get();
            req.id = SYSCALL_REQ_ID_SENDTO;
            req.seq = seq;
            req.buf_off = buf_off;
            req.buf_len = buf_len;
            req.dst_ipv4_be = dst_ipv4_be;
            req.dst_port_be = dst_port_be;
            req.reserved = 0;
        }
    }

    #[inline(always)]
    fn slot_read_req(&self, idx: usize) -> SyscallDelegationReq {
        #[allow(unsafe_code)]
        unsafe {
            *self.slots[idx].req.get()
        }
    }

    #[inline(always)]
    fn slot_store_completion(&self, idx: usize, completion: i64) {
        self.slots[idx]
            .completion
            .value
            .store(completion as u64, Ordering::Release);
    }

    #[inline(always)]
    fn slot_load_completion(&self, idx: usize) -> i64 {
        self.slots[idx]
            .completion
            .value
            .load(Ordering::Acquire) as i64
    }

    #[inline(always)]
    fn slot_completion_vaddr(&self, idx: usize) -> usize {
        (&self.slots[idx].completion.value as *const AtomicU64) as usize
    }

}

static SYSCALL_DELEG_RING_SLOTS: SpinOnce<SyscallDelegationSlots> = SpinOnce::new();

#[inline(always)]
fn syscall_deleg_ring_slots() -> &'static SyscallDelegationSlots {
    SYSCALL_DELEG_RING_SLOTS.call_once(SyscallDelegationSlots::new)
}

pub(crate) static SYSCALL_DELEG_RING_HEAD: CacheLineAtomicU64 = CacheLineAtomicU64::new(0);
pub(crate) static SYSCALL_DELEG_RING_TAIL: CacheLineAtomicU64 = CacheLineAtomicU64::new(0);
pub(crate) static SYSCALL_DELEG_BSP_READY: AtomicU64 = AtomicU64::new(0);
pub(crate) static SYSCALL_DELEG_HANDLER_HART: AtomicU64 = AtomicU64::new(u32::MAX as u64);
pub(crate) static SYSCALL_DELEG_SHARED_BUF: SpinOnce<Segment<()>> = SpinOnce::new();

pub(crate) fn syscall_delegation_enabled() -> bool {
    kernel_cmdline()
        .get_initproc_argv()
        .iter()
        .any(|a| a.as_bytes() == b"syscall_delegation")
}

pub(crate) fn syscall_delegation_init_shared_buffer() -> &'static Segment<()> {
    SYSCALL_DELEG_SHARED_BUF.call_once(|| {
        let nframes = (SYSCALL_SHARED_BUF_BYTES + PAGE_SIZE - 1) / PAGE_SIZE;
        FrameAllocOptions::new()
            .zeroed(true)
            .alloc_segment(nframes)
            .expect("alloc syscall delegation shared buffer failed")
    })
}

#[inline(always)]
pub(crate) fn syscall_deleg_ring_empty() -> bool {
    let tail = SYSCALL_DELEG_RING_TAIL.value.load(Ordering::Relaxed);
    let head = SYSCALL_DELEG_RING_HEAD.value.load(Ordering::Acquire);
    head == tail
}

#[inline(always)]
pub(crate) fn syscall_deleg_ring_full(head: u64, tail: u64) -> bool {
    head.wrapping_sub(tail) >= MAX_SYSCALL_RING_SLOTS as u64
}

#[inline(always)]
pub(crate) fn syscall_deleg_try_dequeue() -> Option<(usize, SyscallDelegationReq)> {
    let tail = SYSCALL_DELEG_RING_TAIL.value.load(Ordering::Relaxed);
    let head = SYSCALL_DELEG_RING_HEAD.value.load(Ordering::Acquire);
    if head == tail {
        return None;
    }
    let slot = (tail as usize) & (MAX_SYSCALL_RING_SLOTS - 1);
    let req = syscall_deleg_ring_slots().slot_read_req(slot);
    SYSCALL_DELEG_RING_TAIL
        .value
        .store(tail.wrapping_add(1), Ordering::Release);
    Some((slot, req))
}

#[inline(always)]
pub(crate) fn syscall_deleg_slot_write_sendto(
    slot_idx: usize,
    seq: u32,
    buf_off: u32,
    buf_len: u32,
    dst_ipv4_be: u32,
    dst_port_be: u16,
) {
    syscall_deleg_ring_slots().slot_write_sendto(
        slot_idx,
        seq,
        buf_off,
        buf_len,
        dst_ipv4_be,
        dst_port_be,
    );
}

#[inline(always)]
pub(crate) fn syscall_deleg_slot_store_completion(slot_idx: usize, completion: i64) {
    syscall_deleg_ring_slots().slot_store_completion(slot_idx, completion);
}

#[inline(always)]
pub(crate) fn syscall_deleg_slot_load_completion(slot_idx: usize) -> i64 {
    syscall_deleg_ring_slots().slot_load_completion(slot_idx)
}

#[inline(always)]
pub(crate) fn syscall_deleg_slot_completion_vaddr(slot_idx: usize) -> usize {
    syscall_deleg_ring_slots().slot_completion_vaddr(slot_idx)
}

pub(crate) fn syscall_delegation_wait_bsp_ready(max_spins: usize) -> bool {
    for _ in 0..max_spins {
        if SYSCALL_DELEG_BSP_READY.load(Ordering::Acquire) != 0 {
            return true;
        }
        #[allow(unsafe_code)]
        unsafe {
            core::arch::asm!("nop", options(nomem, nostack));
        }
    }
    false
}

pub(crate) fn syscall_delegation_fill_payload(
    seq: usize,
    buf: &mut [u8; SYSCALL_SLOT_DATA_BYTES],
) -> usize {
    const PREFIX: &[u8] = b"syscall_delegation seq=";
    let base_len = PREFIX.len();
    buf[..base_len].copy_from_slice(PREFIX);
    let digit = (seq % 10) as u8;
    buf[base_len] = b'0' + digit;
    base_len + 1
}

pub(crate) fn threadlet_user_send_to(
    seq: u32,
    buf_off: u32,
    buf_len: u32,
    dst_ipv4_be: u32,
    dst_port_be: u16,
) -> i64 {
    use ostd::arch::riscv::threadlet;
    threadlet::threadlet_syn_print(SYSCALL_STAGE, 98);

    loop {
        let tail = SYSCALL_DELEG_RING_TAIL.value.load(Ordering::Acquire);
        let head = SYSCALL_DELEG_RING_HEAD.value.load(Ordering::Relaxed);
        if syscall_deleg_ring_full(head, tail) {
            threadlet::threadlet_pass();
            continue;
        }

        let slot = (head as usize) & (MAX_SYSCALL_RING_SLOTS - 1);
        syscall_deleg_slot_store_completion(slot, SYSCALL_COMPLETION_PENDING);
        syscall_deleg_slot_write_sendto(slot, seq, buf_off, buf_len, dst_ipv4_be, dst_port_be);
        SYSCALL_DELEG_RING_HEAD
            .value
            .store(head.wrapping_add(1), Ordering::Release);

        let completion_paddr = kernel_vaddr_to_paddr(syscall_deleg_slot_completion_vaddr(slot));
        loop {
            if let Some(completion_paddr) = completion_paddr {
                threadlet::threadlet_dcache_monitor_set(completion_paddr, 0);
            }

            let completion = syscall_deleg_slot_load_completion(slot);
            if completion != SYSCALL_COMPLETION_PENDING {
                if let Some(completion_paddr) = completion_paddr {
                    threadlet::threadlet_dcache_monitor_clear(completion_paddr, 0);
                }
                threadlet::threadlet_syn_print(SYSCALL_STAGE, 99);
                return completion;
            }
            threadlet::threadlet_yield();
        }
    }
}
