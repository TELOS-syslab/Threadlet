// SPDX-License-Identifier: MPL-2.0

use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use ostd::arch::riscv::threadlet;

use crate::thread::kernel_thread::{threadlet_end, ThreadOptions};

pub const EEVDF_PRIORITY: u32 = 17;
pub const EEVDF_SCHEDULER_STAGE: u64 = 999;

const EEVDF_WORKER_COUNTS: [usize; 3] = [2, 4, 8];
const EEVDF_MAX_WORKERS: usize = 8;
const EEVDF_WORKER_TIMESLICE: u64 = 16_384;
const EEVDF_WORKER_NOPS_PER_SLICE: usize = 1450; // About 10 us.
const EEVDF_DEADLINE_BASE_STEP: u64 = 16_384;
const EEVDF_SCHEDULER_BIG_SLICE: u64 = 1 << 20;

const EEVDF_ROUND_START: u64 = 100;
const EEVDF_SCHEDULER_INIT_END: u64 = 101;
const EEVDF_WORKER_START: u64 = 199;
const EEVDF_WORKER_DONE: u64 = 200;
const EEVDF_SCHEDULER_UPDATE_START: u64 = 300;
const EEVDF_SCHEDULER_UPDATE_END: u64 = 301;
const EEVDF_ROUND_END: u64 = 9999;

static EEVDF_WORKER_HARTS: [AtomicU64; EEVDF_MAX_WORKERS] =
    [const { AtomicU64::new(u32::MAX as u64) }; EEVDF_MAX_WORKERS];
static EEVDF_ACTIVE_WORKERS: AtomicUsize = AtomicUsize::new(0);
static EEVDF_WORKERS_DONE: AtomicUsize = AtomicUsize::new(0);
static EEVDF_ROUND_DONE: AtomicBool = AtomicBool::new(false);

#[inline(always)]
fn spin_nops(times: usize) {
    for _ in 0..times {
        #[allow(unsafe_code)]
        unsafe {
            core::arch::asm!("nop", options(nomem, nostack));
        }
    }
}

pub fn eevdf_worker_threadlet() {
    threadlet::threadlet_syn_print(EEVDF_SCHEDULER_STAGE, EEVDF_WORKER_START);
    spin_nops(EEVDF_WORKER_NOPS_PER_SLICE);
    threadlet::threadlet_syn_print(EEVDF_SCHEDULER_STAGE, EEVDF_WORKER_DONE);
    EEVDF_WORKERS_DONE.fetch_add(1, Ordering::Release);
    threadlet::threadlet_halt();
}

pub fn eevdf_scheduler_threadlet() {
    let worker_count = EEVDF_ACTIVE_WORKERS.load(Ordering::Acquire);


    let mut vruntime = [0u64; EEVDF_MAX_WORKERS];
    let mut deadline_step = [0u64; EEVDF_MAX_WORKERS];
    let mut deadline = [0u64; EEVDF_MAX_WORKERS];
    let mut system_clock = 0;

    threadlet::threadlet_syn_print(EEVDF_SCHEDULER_STAGE, EEVDF_ROUND_START);
    for i in 0..worker_count {
        let step = EEVDF_DEADLINE_BASE_STEP * ((i as u64) + 1);
        deadline[i] = step;
        let hart = EEVDF_WORKER_HARTS[i].load(Ordering::Acquire) as u32;
        threadlet::threadlet_set_timeslice(hart, EEVDF_WORKER_TIMESLICE);
        threadlet::threadlet_set_deadline(hart, deadline[i]);
        threadlet::threadlet_wakeup(hart);
    }
    threadlet::threadlet_syn_print(EEVDF_SCHEDULER_STAGE, EEVDF_SCHEDULER_INIT_END);

    loop {
        threadlet::threadlet_pass();

        system_clock += 1;
        threadlet::threadlet_syn_print(
            EEVDF_SCHEDULER_STAGE,
            EEVDF_SCHEDULER_UPDATE_START,
        );

        for i in 0..worker_count {
            let hart = EEVDF_WORKER_HARTS[i].load(Ordering::Acquire) as u32;
            vruntime[i] = vruntime[i].wrapping_add(EEVDF_WORKER_TIMESLICE);
            if vruntime[i] > deadline[i] {
                deadline[i] = deadline[i].wrapping_add(deadline_step[i]);
            }
            threadlet::threadlet_set_deadline(hart, deadline[i]);
            threadlet::threadlet_set_timeslice(hart, EEVDF_WORKER_TIMESLICE);
        }

        let all_workers_done = EEVDF_WORKERS_DONE.load(Ordering::Acquire) >= worker_count;
        threadlet::threadlet_syn_print(
            EEVDF_SCHEDULER_STAGE,
            EEVDF_SCHEDULER_UPDATE_END,
        );
        if all_workers_done {
            threadlet::threadlet_syn_print(EEVDF_SCHEDULER_STAGE, EEVDF_ROUND_END);
            break;
        }
    }

    EEVDF_ROUND_DONE.store(true, Ordering::Release);
    threadlet::threadlet_halt();
}


pub fn start_eevdf_scheduling_test(cpu_id: usize, ctx_base_paddr: usize) {
    for (round_index, worker_count) in EEVDF_WORKER_COUNTS.into_iter().enumerate() {
        reset_round(worker_count);

        let mut worker_harts = [u32::MAX; EEVDF_MAX_WORKERS];
        for (worker_index, hart_entry) in worker_harts
            .iter_mut()
            .enumerate()
            .take(worker_count)
        {
            let eevdf_id = (worker_index + 1) as u64;
            let (_task, hart_id) = ThreadOptions::threadlet_new_auto_with_arg_with_mem(
                eevdf_worker_threadlet,
                EEVDF_PRIORITY,
                eevdf_id,
                false,
                ctx_base_paddr,
            );
            assert_ne!(hart_id, u32::MAX, "failed to create EEVDF worker threadlet");

            *hart_entry = hart_id;
            EEVDF_WORKER_HARTS[worker_index].store(hart_id as u64, Ordering::Release);
        }

        let (_task, scheduler_hart) = ThreadOptions::threadlet_new_auto_with_arg_with_mem(
            eevdf_scheduler_threadlet,
            EEVDF_PRIORITY,
            0,
            false,
            ctx_base_paddr,
        );
        assert_ne!(
            scheduler_hart,
            u32::MAX,
            "failed to create EEVDF scheduler threadlet"
        );
        threadlet::threadlet_set_timeslice(scheduler_hart, EEVDF_SCHEDULER_BIG_SLICE);


        threadlet::threadlet_wakeup(scheduler_hart);

        while !EEVDF_ROUND_DONE.load(Ordering::Acquire) {
            spin_nops(1);
        }

        // Test threadlets have halted, so their hart IDs can be reused next round.
        threadlet::threadlet_set_priority(0, 30);
        for hart_id in worker_harts.iter().copied().take(worker_count) {
            threadlet_end(cpu_id, hart_id);
        }
        threadlet_end(cpu_id, scheduler_hart);
        ostd::early_println!(
            "[scheduling] round={} workers={} done",
            round_index + 1,
            worker_count
        );
        threadlet::threadlet_set_priority(0, 1);
    }

    for _ in 0..200 {
        threadlet::threadlet_syn_print(EEVDF_SCHEDULER_STAGE, EEVDF_ROUND_END);
    }
}

fn reset_round(worker_count: usize) {
    for hart in EEVDF_WORKER_HARTS.iter() {
        hart.store(u32::MAX as u64, Ordering::Relaxed);
    }
    EEVDF_ACTIVE_WORKERS.store(worker_count, Ordering::Release);
    EEVDF_WORKERS_DONE.store(0, Ordering::Relaxed);
    EEVDF_ROUND_DONE.store(false, Ordering::Release);
}
