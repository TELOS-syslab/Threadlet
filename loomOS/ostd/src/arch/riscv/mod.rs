// SPDX-License-Identifier: MPL-2.0

//! Platform-specific code for the RISC-V platform.

pub mod boot;
pub(crate) mod cpu;
pub mod device;
pub(crate) mod ex_table;
pub mod iommu;
pub(crate) mod irq;
pub(crate) mod mm;
pub(crate) mod pci;
pub mod qemu;
pub mod serial;
pub mod task;
pub mod threadlet;
pub mod timer;
pub mod trap;

use core::{arch::asm, sync::atomic::Ordering};

pub(crate) fn init_on_bsp() {
    // SAFETY: this function is only called once on BSP.
    unsafe {
        trap::init(true);
    }

    crate::early_println!("[ostd::init_on_bsp] try irq init.");
    irq::init();

    crate::smp::init();  

    // SAFETY: they are only called once on BSP and ACPI has been initialized.
    unsafe {
        crate::cpu::init_num_cpus();
        crate::cpu::set_this_cpu_id(0);
    }

    // SAFETY: no CPU local objects have been accessed by this far. And
    // we are on the BSP.
    unsafe { crate::cpu::local::init_on_bsp() };

    crate::early_println!("[ostd::init_on_bsp] try device(plic) init.");
    device::init();

    crate::early_println!("[ostd::init_on_bsp] try  timer init.");
    timer::init();

    crate::early_println!("[ostd::init_on_bsp] try  boot_all_aps.");
    crate::boot::smp::boot_all_aps();


}


pub(crate) unsafe fn init_on_ap() {
    crate::early_println!("[ostd::init_on_ap] Initializing AP...");

    // 1. Enable supervisor-level timer interrupts for this hart.
    timer::init_on_ap();

    // 2. Enable supervisor-level software and external interrupts.
    riscv::register::sie::set_ssoft();

    // enable external interrupts
    riscv::register::sie::set_sext();
}

pub(crate) fn interrupts_ack(irq_number: usize) {
    if irq_number != irq::TIMER_IRQ_LINE {
        device::plic::complete_interrupt(irq_number as u16);
    }
}

/// Reads a hardware generated 64-bit random value.
///
/// Returns None if no random value was generated.
pub fn read_random() -> Option<u64> {
    Some(0x1234)
}
/// Return the frequency of TSC. The unit is Hz.
pub fn tsc_freq() -> u64 {
    timer::TIMEBASE_FREQ.load(Ordering::Relaxed)
}

/// Reads the current value of the processor’s time-stamp counter (TSC).
pub fn read_tsc() -> u64 {
    riscv::register::time::read64()
}

pub fn rdtsc() -> u64 {
    let tsc: u64;
    unsafe {
        asm!(
            "rdcycle {}",
            out(reg) tsc
        );
    }
    tsc
}


pub(crate) fn enable_cpu_features() {
    unsafe {
        riscv::register::sstatus::set_fs(riscv::register::sstatus::FS::Clean);
    }
}
