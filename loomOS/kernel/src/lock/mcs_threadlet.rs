// SPDX-License-Identifier: MPL-2.0

//! MCS blocking-lock test using threadlet notification.
//!
//! Enable with kernel cmdline argument: `enable_mcs_test`
pub const MCS_STAGE: u64 = 20;

#[cfg(target_arch = "riscv64")]
mod imp {
    use core::{
        arch::asm,
        sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    };

    use ostd::{
        arch::threadlet::{
            threadlet_current, threadlet_halt, threadlet_set_priority,
            threadlet_set_timeslice, threadlet_syn_print, threadlet_wakeup,
        },
        boot::kernel_cmdline,
        cpu::{num_cpus, CpuId, PinCurrentCpu},
    };

    use crate::{
        lock::{
            mcs_threadlet::MCS_STAGE,
            mcslock::{
                mcs_hwlog, McsLock, McsNode, MCS_HW_INIT, MCS_HW_PRINT_CRITICAL_ENTER,
                MCS_HW_PRINT_CRITICAL_EXIT, MCS_HW_TEST_DONE,
            },
        },
        thread::kernel_thread::{threadlet_end, ThreadOptions},
    };

    const TEST_THREADLET_PRIO: u32 = 9;
    const MCS_THREAD_COUNTS: [usize; 4] = [1, 2, 4, 8];
    const MCS_MAX_THREADS_PER_CPU: usize = 8;
    const MCS_CRITICAL_ROUNDS: usize = 10;

    static TEST_ROUND: AtomicUsize = AtomicUsize::new(0);
    static TEST_START_ROUND: AtomicUsize = AtomicUsize::new(0);
    static TEST_WAKE_ROUND: AtomicUsize = AtomicUsize::new(0);
    static TEST_EXPECTED_THREADLETS: AtomicUsize = AtomicUsize::new(0);
    static TEST_READY_CPUS: AtomicUsize = AtomicUsize::new(0);
    static TEST_START_READY_CPUS: AtomicUsize = AtomicUsize::new(0);
    static TEST_FINISHED_CPUS: AtomicUsize = AtomicUsize::new(0);
    static TEST_DONE_THREADLETS: AtomicUsize = AtomicUsize::new(0);
    static TEST_ABORT: AtomicBool = AtomicBool::new(false);

    static CPU_WORK_XOR: AtomicU64 = AtomicU64::new(0);

    struct Shared {
        seq: u64,
        buf: [u64; 1024 * 64],
        checksum: u64,
    }

    impl Shared {
        const fn new() -> Self {
            Self {
                seq: 0,
                buf: [0; 1024 * 64],
                checksum: 0,
            }
        }
    }

    static TEST_LOCK: McsLock<Shared> = McsLock::new(Shared::new());

    pub fn enabled() -> bool {
        kernel_cmdline()
            .get_initproc_argv()
            .iter()
            .any(|a| a.as_bytes() == b"enable_mcs_test")
    }

    pub fn ap_run_workload_until_done(cpu: CpuId, ctx_base_paddr: usize) {
        run_all_rounds_on_cpu(cpu, false, ctx_base_paddr);
    }

    pub fn bsp_run_workload_until_done(cpu: CpuId, ctx_base_paddr: usize) {
        run_all_rounds_on_cpu(cpu, true, ctx_base_paddr);
    }

    fn run_all_rounds_on_cpu(cpu: CpuId, is_bsp: bool, ctx_base_paddr: usize) {
        if !enabled() || !ostd::arch::riscv::boot::smp::threadlet_present() {
            return;
        }

        for _ in 0..100_000 * cpu.as_usize() {
            core::hint::spin_loop();
        }

        let expected_cpus = num_cpus();
        for (round_index, thread_count) in MCS_THREAD_COUNTS.into_iter().enumerate() {
            let round = round_index + 1;
            if is_bsp {
                prepare_round(round, thread_count, expected_cpus);
            } else if !wait_for_phase(&TEST_ROUND, round) {
                return;
            }

            let hart_ids = create_round_threadlets(cpu, thread_count, ctx_base_paddr);
            TEST_READY_CPUS.fetch_add(1, Ordering::AcqRel);

            if is_bsp {
                if !wait_for_cpu_barrier(&TEST_READY_CPUS, expected_cpus, round, "ready") {
                    return;
                }
                TEST_START_ROUND.store(round, Ordering::Release);
            }
            if !wait_for_phase(&TEST_START_ROUND, round) {
                return;
            }

            TEST_START_READY_CPUS.fetch_add(1, Ordering::AcqRel);
            if is_bsp {
                if !wait_for_cpu_barrier(
                    &TEST_START_READY_CPUS,
                    expected_cpus,
                    round,
                    "start-ready",
                ) {
                    return;
                }
                threadlet_set_priority(0, 30);
                ostd::early_println!(
                    "-----------------------------[mcs-test][bsp] round={} threads_per_cpu={} start",
                    round,
                    thread_count
                );
                TEST_WAKE_ROUND.store(round, Ordering::Release);
                threadlet_set_priority(0, 1);
            }
            if !wait_for_phase(&TEST_WAKE_ROUND, round) {
                return;
            }

            mcs_hwlog(MCS_HW_INIT);
            for hart_id in hart_ids.iter().copied().take(thread_count) {
                if hart_id != u32::MAX {
                    threadlet_wakeup(hart_id);
                }
            }

            run_control_workload_until_done();

            // Threadlet 0 resumes only after higher-priority local workers halt.
            for hart_id in hart_ids.iter().copied().take(thread_count) {
                if hart_id != u32::MAX {
                    threadlet_end(cpu.as_usize(), hart_id);
                }
            }

            TEST_FINISHED_CPUS.fetch_add(1, Ordering::AcqRel);
            if is_bsp {
                wait_for_finished_cpus(expected_cpus);
                if TEST_ABORT.load(Ordering::Acquire) {
                    return;
                }

                let done = TEST_DONE_THREADLETS.load(Ordering::Acquire);
                let expected = TEST_EXPECTED_THREADLETS.load(Ordering::Acquire);
                let cpu_work = CPU_WORK_XOR.load(Ordering::Relaxed);
                threadlet_set_priority(0, 30);
                threadlet_syn_print(MCS_STAGE, 9999);
                ostd::early_println!(
                    "------------------------[mcs-test][bsp] round={} done: done_threadlets={} expected_threadlets={} cpu_work_xor={:#x}",
                    round,
                    done,
                    expected,
                    cpu_work
                );
                threadlet_set_priority(0, 1);
            }
        }

        if is_bsp {
            for _ in 0..200 {
                threadlet_syn_print(MCS_STAGE, 9999);
            }
        }
    }

    fn prepare_round(round: usize, thread_count: usize, expected_cpus: usize) {
        TEST_READY_CPUS.store(0, Ordering::Relaxed);
        TEST_START_READY_CPUS.store(0, Ordering::Relaxed);
        TEST_FINISHED_CPUS.store(0, Ordering::Relaxed);
        TEST_DONE_THREADLETS.store(0, Ordering::Relaxed);
        CPU_WORK_XOR.store(0, Ordering::Relaxed);
        TEST_EXPECTED_THREADLETS.store(expected_cpus * thread_count, Ordering::Relaxed);
        TEST_ROUND.store(round, Ordering::Release);
    }

    fn create_round_threadlets(
        cpu: CpuId,
        thread_count: usize,
        ctx_base_paddr: usize,
    ) -> [u32; MCS_MAX_THREADS_PER_CPU] {
        let mut hart_ids = [u32::MAX; MCS_MAX_THREADS_PER_CPU];
        for (slot, hart_id_entry) in hart_ids.iter_mut().enumerate().take(thread_count) {
            let (_task, hart_id) = ThreadOptions::threadlet_new_auto_with_arg_with_mem(
                mcs_test_threadlet_entry,
                TEST_THREADLET_PRIO,
                slot as u64,
                false,
                ctx_base_paddr,
            );
            if hart_id == u32::MAX {
                TEST_ABORT.store(true, Ordering::Release);
                ostd::early_println!(
                    "[mcs-test][cpu={}] failed to create test threadlet: no free hart",
                    cpu.as_usize()
                );
            } else {
                *hart_id_entry = hart_id;
                threadlet_set_timeslice(hart_id, 65536);
            }
        }
        hart_ids
    }

    fn wait_for_phase(phase: &AtomicUsize, expected: usize) -> bool {
        while phase.load(Ordering::Acquire) < expected
            && !TEST_ABORT.load(Ordering::Acquire)
        {
            core::hint::spin_loop();
        }
        !TEST_ABORT.load(Ordering::Acquire)
    }

    fn wait_for_cpu_barrier(
        counter: &AtomicUsize,
        expected: usize,
        round: usize,
        name: &str,
    ) -> bool {
        const SPIN_MAX: usize = 50_000_000;
        for _ in 0..SPIN_MAX {
            if TEST_ABORT.load(Ordering::Acquire) {
                return false;
            }
            if counter.load(Ordering::Acquire) >= expected {
                return true;
            }
            core::hint::spin_loop();
        }
        TEST_ABORT.store(true, Ordering::Release);
        ostd::early_println!(
            "[mcs-test][bsp] round={} timeout at {} barrier: arrived={} expected={}",
            round,
            name,
            counter.load(Ordering::Acquire),
            expected
        );
        false
    }

    fn wait_for_finished_cpus(expected: usize) {
        while TEST_FINISHED_CPUS.load(Ordering::Acquire) < expected
            && !TEST_ABORT.load(Ordering::Acquire)
        {
            core::hint::spin_loop();
        }
    }

    fn run_control_workload_until_done() {
        while !test_finished() {
            for _ in 0..1_000 {
                #[allow(unsafe_code)]
                unsafe {
                    asm!("nop", options(nomem, nostack));
                }
            }
        }
    }

    fn test_finished() -> bool {
        if TEST_ABORT.load(Ordering::Acquire) {
            return true;
        }
        let expected = TEST_EXPECTED_THREADLETS.load(Ordering::Acquire);
        expected != 0 && TEST_DONE_THREADLETS.load(Ordering::Acquire) >= expected
    }

    fn mcs_test_threadlet_entry() {
        let threadlet_id = threadlet_current() as u32;
        let preempt_guard = ostd::task::disable_preempt();
        let cpu_id = preempt_guard.current_cpu().as_usize();
        drop(preempt_guard);
        mcs_hwlog(cpu_id as u64);

        // Allocate the node from the linear-mapped kernel heap for D$ monitoring.
        let mut node = McsNode::boxed();
        node.set_owner_info(cpu_id, threadlet_id);
        mcs_hwlog(cpu_id as u64);

        for _ in 0..MCS_CRITICAL_ROUNDS {
            let _guard = TEST_LOCK.lock_threadlet(&mut *node);
            mcs_hwlog(MCS_HW_PRINT_CRITICAL_ENTER);
            // A critical section of about 10 μs.
            for _ in 0..1450 {
                #[allow(unsafe_code)]
                unsafe {
                    asm!("nop", options(nomem, nostack));
                }
            }
            mcs_hwlog(MCS_HW_PRINT_CRITICAL_EXIT);
        }

        drop(node);
        TEST_DONE_THREADLETS.fetch_add(1, Ordering::AcqRel);
        mcs_hwlog(MCS_HW_TEST_DONE);
        threadlet_halt();
    }
}

#[cfg(not(target_arch = "riscv64"))]
mod imp {
    use ostd::cpu::CpuId;

    pub fn enabled() -> bool {
        false
    }
    pub fn ap_run_workload_until_done(_cpu: CpuId, _ctx_base_paddr: usize) {}
    pub fn bsp_run_workload_until_done(_cpu: CpuId, _ctx_base_paddr: usize) {}
}

pub use imp::*;
