// SPDX-License-Identifier: MPL-2.0

//! The timer support.

use core::sync::atomic::{AtomicU64, Ordering};

use spin::Once;

use crate::{
    arch::{
        boot::{smp, DEVICE_TREE},
        device::create_device_io_mem,
        irq::TIMER_IRQ_LINE,
    },
    io_mem::IoMem,
    timer::INTERRUPT_CALLBACKS,
    trap::{self, IrqLine, TrapFrame},
};

/// The timer frequency (Hz). Here we choose 1000Hz since 1000Hz is easier for unit conversion and
/// convenient for timer. What's more, the frequency cannot be set too high or too low, 1000Hz is
/// a modest choice.
///
/// For system performance reasons, this rate cannot be set too high, otherwise most of the time
/// is spent executing timer code.
pub const TIMER_FREQ: u64 = 1;

pub(crate) static TIMEBASE_FREQ: AtomicU64 = AtomicU64::new(1);
static TIMER_STEP: AtomicU64 = AtomicU64::new(1);
static TIMER_IRQ: Once<IrqLine> = Once::new();

/// [`IoMem`] of goldfish RTC, which will be used by `aster-time`.
pub static GOLDFISH_IO_MEM: Once<IoMem> = Once::new();

pub(super) fn init() {
    init_timer();
    init_rtc();
}

// add ap timer init
pub(super) fn init_on_ap() {
    if smp::threadlet_present() {
        // In threadlet mode, keep timer interrupt masked to avoid periodic timer traps.
        unsafe {
            riscv::register::sie::clear_stimer();
        }
        return;
    }

    set_next_timer();
    unsafe {
        riscv::register::sie::set_stimer();
    }
}

fn init_timer() {
    let timer_freq = DEVICE_TREE
        .get()
        .unwrap()
        .cpus()
        .next()
        .unwrap()
        .timebase_frequency() as u64;
    let timer_step = timer_freq / TIMER_FREQ;
    TIMEBASE_FREQ.store(timer_freq, Ordering::Relaxed);
    TIMER_STEP.store(timer_step, Ordering::Relaxed);

    let disable_timer_irq = smp::threadlet_present();
    set_next_timer();
    unsafe {
        if disable_timer_irq {
            // In threadlet mode, tmp disable timer interrupt 
            riscv::register::sie::clear_stimer();
        } else {
            riscv::register::sie::set_stimer();
        }
    }

    let mut irq = IrqLine::alloc_specific(TIMER_IRQ_LINE as u8).unwrap();
    irq.on_active(timer_callback);
    TIMER_IRQ.call_once(|| irq);

    log::debug!("Timer initialized with frequency: {timer_freq} Hz, timer step: {timer_step} Hz",);
    crate::early_print!("Timer initialized with frequency: {timer_freq} Hz, timer step: {timer_step} Hz\n", timer_freq = timer_freq, timer_step = timer_step);
    if disable_timer_irq {
        crate::early_print!("[timer] threadlet detected from FDT, supervisor timer interrupt is disabled\n");
    }
}

fn set_next_timer() {
    let timer_step = TIMER_STEP.load(Ordering::Relaxed);
    let now = riscv::register::time::read64();
    sbi_rt::set_timer(now + timer_step);
}

pub(crate) fn timer_callback(_: &TrapFrame) {
    crate::timer::jiffies::ELAPSED.fetch_add(1, Ordering::SeqCst);

    let irq_guard = trap::disable_local();
    let callbacks_guard = INTERRUPT_CALLBACKS.get_with(&irq_guard);
    for callback in callbacks_guard.borrow().iter() {
        (callback)();
    }
    drop(callbacks_guard);

    unsafe {
        riscv::register::sie::clear_stimer();
    }
}

fn init_rtc() {
    if let Some(node) = DEVICE_TREE.get().and_then(|f| f.find_node("/soc/rtc")) {
        if let Some(compatible) = node.compatible()
            && compatible.all().any(|c| c == "google,goldfish-rtc")
        {
            if let Some(mut regs) = node.reg() {
                if let Some(region) = regs.next() {
                    let io_mem = unsafe {
                        create_device_io_mem(region.starting_address, region.size.unwrap_or(0))
                    };
                    GOLDFISH_IO_MEM.call_once(|| io_mem);
                }
            }
        }
    }
}
