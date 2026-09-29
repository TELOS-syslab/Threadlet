// SPDX-License-Identifier: MPL-2.0

//! An MCS queue lock for the kernel.
//!
//! This module provides two locking APIs:
//! - `lock_spin`: pure spinning (no threadlet instructions).
//! - `lock_threadlet`: hybrid spin + threadlet D$ monitor + `threadlet_yield()` (RISC-V only).
//!
//! # Usage notes
//!
//! - Each contending thread must provide a distinct [`McsNode`] while holding the lock.
//! - For threadlet blocking mode, the wait flag lives in a dedicated 64B cacheline so that the
//!   monitored cacheline is not shared with other metadata.
//! - Spurious wakeups are allowed by the hardware design; the implementation always re-checks the
//!   wait flag after waking up.

#![allow(unsafe_code)]

use alloc::boxed::Box;
use core::{
    cell::UnsafeCell,
    ops::{Deref, DerefMut},
    ptr,
    sync::atomic::{fence, AtomicPtr, AtomicU64, AtomicU8, Ordering},
};

#[cfg(target_arch = "riscv64")]
use ostd::arch::riscv::threadlet;

/// Base value for MCS HW log tags.
pub const MCS_HW_PRINT_START: u64 = 2000;

// Event tags: `threadlet_syn_print(stage, TAG)`
pub const MCS_HW_PRINT_CRITICAL_EXIT: u64 = MCS_HW_PRINT_START + 0;
pub const MCS_HW_PRINT_UNLOCK_BEGIN: u64 = MCS_HW_PRINT_START + 1;
pub const MCS_HW_PRINT_UNLOCK_STORE: u64 = MCS_HW_PRINT_START + 2;
pub const MCS_HW_PRINT_UNLOCK_DONE: u64 = MCS_HW_PRINT_START + 3;
pub const MCS_HW_PRINT_WAIT_ARM: u64 = MCS_HW_PRINT_START + 4;
pub const MCS_HW_PRINT_WAIT_YIELD: u64 = MCS_HW_PRINT_START + 5;
pub const MCS_HW_PRINT_WAIT_WAKE: u64 = MCS_HW_PRINT_START + 6;
pub const MCS_HW_PRINT_WAIT_DONE: u64 = MCS_HW_PRINT_START + 7;
pub const MCS_HW_PRINT_LOCK_TRY: u64 = MCS_HW_PRINT_START + 8;
pub const MCS_HW_PRINT_LOCK_ACQUIRED: u64 = MCS_HW_PRINT_START + 9;
pub const MCS_HW_PRINT_CRITICAL_ENTER: u64 = MCS_HW_PRINT_START + 10;
pub const MCS_HW_TEST_DONE: u64 = MCS_HW_PRINT_START + 11;
pub const MCS_HW_INIT: u64 = MCS_HW_PRINT_START + 100;
pub const MCS_ABNORMAL: u64 = MCS_HW_PRINT_START + 200;
pub const MCS_STAGE: u64 = 20;

#[inline(always)]
pub(crate) fn mcs_hwlog(tag: u64) {
    #[cfg(all(target_arch = "riscv64", mcs_hw_log))]
    {
        threadlet::threadlet_syn_print(MCS_STAGE, tag);
    }
    #[cfg(not(all(target_arch = "riscv64", mcs_hw_log)))]
    {
        let _ = tag;
    }
}

/// A dedicated cacheline for the wait flag.
///
/// The wait flag is written by the predecessor on unlock, and polled/monitored by the waiter.
#[repr(C, align(64))]
pub struct WaitLine {
    state: AtomicU8,
    _pad: [u8; 63],
}

impl WaitLine {
    const LOCKED: u8 = 1;
    const UNLOCKED: u8 = 0;

    pub const fn new_unlocked() -> Self {
        Self {
            state: AtomicU8::new(Self::UNLOCKED),
            _pad: [0; 63],
        }
    }

    #[inline(always)]
    fn set_locked(&self) {
        self.state.store(Self::LOCKED, Ordering::Relaxed);
    }

    #[inline(always)]
    fn set_unlocked(&self) {
        // Release is required to publish the critical section to the successor.
        self.state.store(Self::UNLOCKED, Ordering::Release);
    }

    #[inline(always)]
    fn is_locked_relaxed(&self) -> bool {
        self.state.load(Ordering::Relaxed) != Self::UNLOCKED
    }

    #[inline(always)]
    fn is_locked_acquire(&self) -> bool {
        self.state.load(Ordering::Acquire) != Self::UNLOCKED
    }
}

/// Per-thread (or per-threadlet) node used by [`McsLock`].
///
/// A node must not be used concurrently by multiple threads.
#[repr(C, align(64))]
pub struct McsNode {
    wait: WaitLine,
    next: AtomicPtr<McsNode>,
    owner_cpu_id: AtomicU64,
    owner_threadlet_id: AtomicU64,
}

impl McsNode {
    pub const fn new() -> Self {
        Self {
            wait: WaitLine::new_unlocked(),
            next: AtomicPtr::new(ptr::null_mut()),
            owner_cpu_id: AtomicU64::new(u64::MAX),
            owner_threadlet_id: AtomicU64::new(u64::MAX),
        }
    }

    pub fn boxed() -> Box<Self> {
        Box::new(Self::new())
    }

    #[inline(always)]
    fn prepare_for_lock(&self) {
        self.next.store(ptr::null_mut(), Ordering::Relaxed);
        self.wait.set_locked();
    }

    #[inline(always)]
    fn wait_flag_vaddr(&self) -> usize {
        (&self.wait.state as *const AtomicU8) as usize
    }

    #[inline(always)]
    pub fn set_owner_info(&self, cpu_id: usize, threadlet_id: u32) {
        self.owner_cpu_id.store(cpu_id as u64, Ordering::Relaxed);
        self.owner_threadlet_id
            .store(threadlet_id as u64, Ordering::Relaxed);
    }

    #[inline(always)]
    fn owner_cpu_id(&self) -> u64 {
        self.owner_cpu_id.load(Ordering::Relaxed)
    }

    #[inline(always)]
    fn owner_threadlet_id(&self) -> u32 {
        self.owner_threadlet_id.load(Ordering::Relaxed) as u32
    }
}

/// An MCS lock protecting `T`.
pub struct McsLock<T: ?Sized> {
    tail: AtomicPtr<McsNode>,
    data: UnsafeCell<T>,
}

unsafe impl<T: ?Sized + Send> Sync for McsLock<T> {}
unsafe impl<T: ?Sized + Send> Send for McsLock<T> {}

impl<T> McsLock<T> {
    pub const fn new(value: T) -> Self {
        Self {
            tail: AtomicPtr::new(ptr::null_mut()),
            data: UnsafeCell::new(value),
        }
    }

    pub fn into_inner(self) -> T {
        self.data.into_inner()
    }
}

impl<T: ?Sized> McsLock<T> {
    /// Acquire the lock with pure spinning.
    pub fn lock_spin<'a>(&'a self, node: &'a mut McsNode) -> McsGuard<'a, T> {
        self.lock_common(node, WaitMode::Spin)
    }

    /// Acquire the lock using hybrid spinning + threadlet notification (RISC-V only).
    ///
    /// On non-RISC-V targets, this falls back to [`McsLock::lock_spin`].
    pub fn lock_threadlet<'a>(&'a self, node: &'a mut McsNode) -> McsGuard<'a, T> {
        self.lock_common(node, WaitMode::Threadlet)
    }

    fn lock_common<'a>(&'a self, node: &'a mut McsNode, mode: WaitMode) -> McsGuard<'a, T> {
        mcs_hwlog(MCS_HW_PRINT_LOCK_TRY);
        node.prepare_for_lock();
        let node_ptr = node as *mut McsNode;
        let pred = self.tail.swap(node_ptr, Ordering::AcqRel);

        if !pred.is_null() {
            let pred = unsafe { &*pred };
            pred.next.store(node_ptr, Ordering::Release);

            match mode {
                WaitMode::Spin => spin_wait(&node.wait),
                WaitMode::Threadlet => threadlet_wait(node),
            }

            // Pair with the predecessor's `Release` store when it unlocks.
            fence(Ordering::Acquire);
            mcs_hwlog(MCS_HW_PRINT_LOCK_ACQUIRED);
        } else {
            // No predecessor: we own the lock immediately.
            node.wait.state.store(WaitLine::UNLOCKED, Ordering::Relaxed);
            mcs_hwlog(MCS_HW_PRINT_LOCK_ACQUIRED);
        }

        McsGuard { lock: self, node }
    }

    fn unlock(&self, node: &McsNode) {
        mcs_hwlog(MCS_HW_PRINT_UNLOCK_BEGIN);
        let node_ptr = node as *const _ as *mut McsNode;
        let mut succ = node.next.load(Ordering::Relaxed);

        if succ.is_null() {
            // Try to reset tail if there is no successor.
            if self
                .tail
                .compare_exchange(node_ptr, ptr::null_mut(), Ordering::Release, Ordering::Relaxed)
                .is_ok()
            {
                return;
            }

            // Someone is enqueuing but hasn't linked itself yet. Spin until linked.
            loop {
                succ = node.next.load(Ordering::Relaxed);
                if !succ.is_null() {
                    break;
                }
                core::hint::spin_loop();
            }
        }

        let succ = unsafe { &*succ };
        mcs_hwlog(MCS_HW_PRINT_UNLOCK_STORE);
        succ.wait.set_unlocked();
        #[cfg(target_arch = "riscv64")]
        {
            let cur_cpu = node.owner_cpu_id();
            let next_cpu = succ.owner_cpu_id();
            let next_threadlet = succ.owner_threadlet_id();
            if  cur_cpu == next_cpu{
                threadlet::threadlet_wakeup(next_threadlet);
            }
        }
        mcs_hwlog(MCS_HW_PRINT_UNLOCK_DONE);
    } 
}

pub struct McsGuard<'a, T: ?Sized + 'a> {
    lock: &'a McsLock<T>,
    node: &'a McsNode,
}

impl<'a, T: ?Sized> Deref for McsGuard<'a, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.lock.data.get() }
    }
}

impl<'a, T: ?Sized> DerefMut for McsGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<'a, T: ?Sized> Drop for McsGuard<'a, T> {
    fn drop(&mut self) {
        self.lock.unlock(self.node);
    }
}

#[derive(Clone, Copy)]
enum WaitMode {
    Spin,
    Threadlet,
}

fn spin_wait(wait: &WaitLine) {
    while wait.is_locked_relaxed() {
        core::hint::spin_loop();
    }
}

#[cfg(not(target_arch = "riscv64"))]
fn threadlet_wait(node: &McsNode) {
    let _ = node;
    // No threadlet instructions on this target; fall back to spinning.
    spin_wait(&node.wait);
}

#[cfg(target_arch = "riscv64")]
fn threadlet_wait(node: &McsNode) {
    const SPIN_LIMIT: usize = 1;
    const MONITOR_SLOT: usize = 0;

    for _ in 0..SPIN_LIMIT {
        if !node.wait.is_locked_relaxed() {
                mcs_hwlog(MCS_HW_PRINT_WAIT_DONE);
                return;
            }
        core::hint::spin_loop();
    }

    while node.wait.is_locked_relaxed() {
        if !node.wait.is_locked_relaxed() {
            mcs_hwlog(MCS_HW_PRINT_WAIT_DONE);
            return;
        }


        let vaddr = node.wait_flag_vaddr();
        let Some(paddr) = crate::kernel_vaddr_to_paddr(vaddr) else {
            // Unexpected: cannot translate the wait flag address. 
            continue;
        };

  


        // Final re-check to close the "arm -> yield" window.
        loop{
            threadlet::threadlet_dcache_monitor_set(paddr, MONITOR_SLOT);
            mcs_hwlog(MCS_HW_PRINT_WAIT_ARM);
            if node.wait.is_locked_acquire() {
                mcs_hwlog(MCS_HW_PRINT_WAIT_YIELD);
                threadlet::threadlet_yield();
                mcs_hwlog(MCS_HW_PRINT_WAIT_WAKE);
                // After waking up, the monitor is cleared by hardware (and spurious wakeups are
                // possible), so we simply loop and re-check / re-arm if needed.
                continue;
            }
            else{
                break;
            }
        }




        threadlet::threadlet_dcache_monitor_clear(paddr, MONITOR_SLOT);
        mcs_hwlog(MCS_HW_PRINT_WAIT_DONE);
        return;
    }
}
