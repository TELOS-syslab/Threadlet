// SPDX-License-Identifier: MPL-2.0

//! Threadlet arch-specific helper routines for kernel fast paths.

#![allow(unsafe_code)]

use core::sync::atomic::{AtomicU64, Ordering};

use crate::thread::kernel_thread::ThreadOptions;
use ostd::{
    arch::threadlet::{threadlet_set_priority, threadlet_set_timeslice, threadlet_syn_print},
    boot::kernel_cmdline,
    cpu::CpuId,
};

#[repr(C, align(64))]
struct IcenetPollingGprTable {
    regs: [[u64; ICENET_POLLING_GPR_COUNT]; ICENET_POLLING_GPR_GROUPS],
}

const ICENET_POLLING_GPR_GROUPS: usize = 8;
const ICENET_POLLING_GPR_COUNT: usize = 32;

// Simulate wakeup restore overhead for the polling threadlet:
// load one of 8 distinct GPR sets, 32x64-bit per call.
static mut ICENET_POLLING_GPR_TABLE: IcenetPollingGprTable = IcenetPollingGprTable {
    regs: [[0u64; ICENET_POLLING_GPR_COUNT]; ICENET_POLLING_GPR_GROUPS],
};

// BSP-only execution: rotate the source GPR set as (i + 1) % 8.
static mut ICENET_POLLING_GPR_GROUP_SEQ: usize = 0;

const REG_STAGE: u64 = 505;

const CTX_SAVER_TEST_MAX_THREADLETS: usize = 400;
const CTX_SAVER_TEST_HIGH_THREADLETS: usize = 10;
const CTX_SAVER_TEST_HIGH_FIRST_ROLE: usize =
    CTX_SAVER_TEST_MAX_THREADLETS - CTX_SAVER_TEST_HIGH_THREADLETS;
const CTX_SAVER_TEST_LOW_PRIO: u32 = 8;
const CTX_SAVER_TEST_HIGH_PRIO: u32 = 16;
const CTX_SAVER_TEST_TIMESLICE: u64 = 4096;
const CTX_SAVER_TEST_WAIT_SPINS: usize = 200;
const CTX_SAVER_TEST_LOW_WORK_CYCLES: usize = 1000;
const CTX_SAVER_TEST_TICKET_WAIT_CYCLES: usize = 8;
const CTX_SAVER_TEST_STAGE: u64 = 700;
const CTX_SAVER_TEST_WAKEUP_PRINT_BASE: u64 = 700_000;
const CTX_SAVER_TEST_START_PRINT_BASE: u64 = 710_000;
static CTX_SAVER_TEST_CREATED_COUNT: AtomicU64 = AtomicU64::new(0);
static CTX_SAVER_TEST_HARTS: [AtomicU64; CTX_SAVER_TEST_MAX_THREADLETS] =
    [const { AtomicU64::new(u32::MAX as u64) }; CTX_SAVER_TEST_MAX_THREADLETS];
static CTX_SAVER_TEST_HIGH_STARTED: [AtomicU64; CTX_SAVER_TEST_HIGH_THREADLETS] =
    [const { AtomicU64::new(0) }; CTX_SAVER_TEST_HIGH_THREADLETS];

pub(crate) fn ctx_saver_test_enabled() -> bool {
    kernel_cmdline()
        .get_initproc_argv()
        .iter()
        .any(|a| a.as_bytes() == b"ctx_saver_test")
}

#[inline(always)]
fn ctx_saver_test_nop_cycles(cycles: usize) {
    for _ in 0..cycles {
        unsafe {
            core::arch::asm!("nop", options(nomem, nostack));
        }
    }
}

fn ctx_saver_test_threadlet_entry() {
    use ostd::arch::riscv::threadlet;

    let role = threadlet::threadlet_get_a0() as usize;

    if role == 0 {
        threadlet::threadlet_syn_print(CTX_SAVER_TEST_STAGE, 1000);
        let created_count = CTX_SAVER_TEST_CREATED_COUNT.load(Ordering::Acquire) as usize;
        threadlet::threadlet_syn_print(CTX_SAVER_TEST_STAGE, 1100);

        for idx in 1..created_count.min(CTX_SAVER_TEST_HIGH_FIRST_ROLE) {
            let hart = CTX_SAVER_TEST_HARTS[idx].load(Ordering::Acquire) as u32;
            threadlet::threadlet_wakeup(hart);
        }
    }

    if role < CTX_SAVER_TEST_HIGH_FIRST_ROLE {
        if role != 0 {
            threadlet::threadlet_syn_print(CTX_SAVER_TEST_STAGE, 1000 + role as u64);
        }
        let high_index = role % CTX_SAVER_TEST_HIGH_THREADLETS;
        let ticket = (role / CTX_SAVER_TEST_HIGH_THREADLETS) as u64;

        ctx_saver_test_nop_cycles(CTX_SAVER_TEST_LOW_WORK_CYCLES);
        let high_role = CTX_SAVER_TEST_HIGH_FIRST_ROLE + high_index;
        let high_hart = CTX_SAVER_TEST_HARTS[high_role].load(Ordering::Acquire) as u32;
        threadlet::threadlet_syn_print(
            CTX_SAVER_TEST_STAGE,
            CTX_SAVER_TEST_WAKEUP_PRINT_BASE + high_hart as u64,
        );
        threadlet::threadlet_wakeup(high_hart);
    } else {
        loop {
            let high_index = role - CTX_SAVER_TEST_HIGH_FIRST_ROLE;
            let high_hart = CTX_SAVER_TEST_HARTS[role].load(Ordering::Acquire);
            threadlet::threadlet_syn_print(
                    CTX_SAVER_TEST_STAGE,
                    CTX_SAVER_TEST_START_PRINT_BASE + high_hart,
            );
            threadlet::threadlet_yield();
        }
    }

    loop {
        threadlet::threadlet_yield();
    }
}

pub(crate) fn run_ctx_saver_test(cpu_id: CpuId, ctx_base_paddr: usize) {
    use ostd::arch::riscv::threadlet;

    let mut harts = [u32::MAX; CTX_SAVER_TEST_MAX_THREADLETS];
    CTX_SAVER_TEST_CREATED_COUNT.store(0, Ordering::Relaxed);
    for hart in CTX_SAVER_TEST_HARTS.iter() {
        hart.store(u32::MAX as u64, Ordering::Relaxed);
    }
    for started in CTX_SAVER_TEST_HIGH_STARTED.iter() {
        started.store(0, Ordering::Relaxed);
    }

    let mut created = 0usize;
    while created < CTX_SAVER_TEST_MAX_THREADLETS {
        let role = created;
        let priority = if role >= CTX_SAVER_TEST_HIGH_FIRST_ROLE {
            CTX_SAVER_TEST_HIGH_PRIO
        } else {
            CTX_SAVER_TEST_LOW_PRIO
        };
        let (_thread, hart_id) = ThreadOptions::threadlet_new_auto_with_arg_with_mem(
            ctx_saver_test_threadlet_entry,
            priority,
            role as u64,
            false,
            ctx_base_paddr,
        );
        if hart_id == u32::MAX {
            ostd::early_println!(
                "[ctx-saver-test] failed to create test threadlet cpu={} role={}",
                cpu_id.as_usize(),
                role
            );
            return;
        }
        harts[role] = hart_id;
        CTX_SAVER_TEST_HARTS[role].store(hart_id as u64, Ordering::Release);
        threadlet_set_timeslice(hart_id, CTX_SAVER_TEST_TIMESLICE);
        if (role == 0){
            threadlet_set_priority(hart_id, CTX_SAVER_TEST_LOW_PRIO+1);
        }

        created += 1;
    }

    let created_count = created;
    CTX_SAVER_TEST_CREATED_COUNT.store(created_count as u64, Ordering::Release);
    ostd::early_println!(
        "[ctx-saver-test] wakeup setup cpu={} created={} low_roles=0..{} high_roles={}..{} high_harts={:?}",
        cpu_id.as_usize(),
        created_count,
        CTX_SAVER_TEST_HIGH_FIRST_ROLE - 1,
        CTX_SAVER_TEST_HIGH_FIRST_ROLE,
        CTX_SAVER_TEST_MAX_THREADLETS - 1,
        &harts[CTX_SAVER_TEST_HIGH_FIRST_ROLE..],
    );

    threadlet_syn_print(CTX_SAVER_TEST_STAGE, 0);

    threadlet::threadlet_wakeup(harts[0]);

    for _ in 0..CTX_SAVER_TEST_WAIT_SPINS {
        ctx_saver_test_nop_cycles(10);
    }

    ostd::early_println!(
        "[ctx-saver-test] DONE cpu={} created={} harts={:?}",
        cpu_id.as_usize(),
        created_count,
        harts
    );
}

#[inline(always)]
pub(crate) fn init_icenet_polling_threadlet_gpr_table() {
    unsafe {
        let mut group = 0;
        while group < ICENET_POLLING_GPR_GROUPS {
            let mut reg = 0;
            while reg < ICENET_POLLING_GPR_COUNT {
                ICENET_POLLING_GPR_TABLE.regs[group][reg] =
                    ((group as u64 + 1) << 56) | ((reg as u64) << 8) | 0x5a;
                reg += 1;
            }
            group += 1;
        }
        ICENET_POLLING_GPR_GROUP_SEQ = 0;
    }
}

#[inline(always)]
pub(crate) fn icenet_polling_threadlet_read_gprs() {
    let group_idx = unsafe {
        let next = (ICENET_POLLING_GPR_GROUP_SEQ + 1) & (ICENET_POLLING_GPR_GROUPS - 1);
        ICENET_POLLING_GPR_GROUP_SEQ = next;
        next
    };
    let base = unsafe { ICENET_POLLING_GPR_TABLE.regs[group_idx].as_ptr() };
    threadlet_syn_print(REG_STAGE, 0);
    unsafe {
        core::arch::asm!(
            // Simulate loading 32 GPRs with 4 bursts of 8 loads.
            "ld a0,   0(t0)",
            "ld a1,   8(t0)",
            "ld a2,  16(t0)",
            "ld a3,  24(t0)",
            "ld a4,  32(t0)",
            "ld a5,  40(t0)",
            "ld a6,  48(t0)",
            "ld a7,  56(t0)",
            "ld a0,  64(t0)",
            "ld a1,  72(t0)",
            "ld a2,  80(t0)",
            "ld a3,  88(t0)",
            "ld a4,  96(t0)",
            "ld a5, 104(t0)",
            "ld a6, 112(t0)",
            "ld a7, 120(t0)",
            "ld a0, 128(t0)",
            "ld a1, 136(t0)",
            "ld a2, 144(t0)",
            "ld a3, 152(t0)",
            "ld a4, 160(t0)",
            "ld a5, 168(t0)",
            "ld a6, 176(t0)",
            "ld a7, 184(t0)",
            "ld a0, 192(t0)",
            "ld a1, 200(t0)",
            "ld a2, 208(t0)",
            "ld a3, 216(t0)",
            "ld a4, 224(t0)",
            "ld a5, 232(t0)",
            "ld a6, 240(t0)",
            "ld a7, 248(t0)",
            in("t0") base,
            out("a0") _,
            out("a1") _,
            out("a2") _,
            out("a3") _,
            out("a4") _,
            out("a5") _,
            out("a6") _,
            out("a7") _,
            options(nostack, readonly)
        );
        threadlet_syn_print(REG_STAGE, 1);
    }
}


#[inline(always)]
pub(crate) fn do_nop_cycle_test() {
    let tsc_start = ostd::arch::rdtsc();
    for ii in 0..1000 {
        #[allow(unsafe_code)]
		    unsafe {
		        core::arch::asm!("nop", options(nomem, nostack));
            }
    }
    let tsc_end = ostd::arch::rdtsc();
    let tsc_delta = tsc_end.wrapping_sub(tsc_start);
    ostd::early_println!("[init] nop(10000) cycles={}", tsc_delta);
}
