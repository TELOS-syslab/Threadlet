// SPDX-License-Identifier: MPL-2.0

//! Handles trap.

mod trap;

use riscv::register::sip;
use riscv::register::scause::{Exception, Interrupt, Trap};
pub use trap::{GeneralRegs, TrapFrame, UserContext};
pub use trap::{trap_threadlet_vector_base, trap_vector_base};
use sbi_rt::{self};

use super::ex_table::ExTable;
use crate::arch::threadlet::threadlet_syn_print;
use crate::early_println;
use crate::{
    arch::irq::TIMER_IRQ_LINE, cpu::CpuExceptionInfo, cpu_local_cell, mm::MAX_USERSPACE_VADDR,
    task::Task,
    trap::{call_irq_callback_functions, call_irq_callback_functions_threadlet},
};

cpu_local_cell! {
    static IS_KERNEL_INTERRUPTED: bool = false;
}

/// Initialize interrupt handling on RISC-V.
pub unsafe fn init(on_bsp: bool) {
    self::trap::init(on_bsp);
}

/// Returns true if this function is called within the context of an IRQ handler
/// and the IRQ occurs while the CPU is executing in the kernel mode.
/// Otherwise, it returns false.
pub fn is_kernel_interrupted() -> bool {
    IS_KERNEL_INTERRUPTED.load()
}

pub fn handle_external_interrupts(f: &TrapFrame) {
    threadlet_syn_print(5, 103);
    while let Some(irq) = super::device::plic::claim_interrupt() {
        call_irq_callback_functions(f, irq.get() as usize);
    }
}

pub fn handle_external_interrupts_threadlet(f: &TrapFrame) {
    threadlet_syn_print(5, 103);
    while let Some(irq) = super::device::plic::claim_interrupt() {
        call_irq_callback_functions_threadlet(f, irq.get() as usize);
    }
}




pub fn handle_software_interrupts(f: &TrapFrame) {
    // crate::early_println!("[ostd::handle_software_interrupts]Try to handle_software_interrupts!");
    
    // sbi_rt::legacy::clear_ipi();
    // Minimal change: do not read MHARTID in S-mode; just log the action.
    crate::early_println!("[handle_software_interrupts] clear soft");
    unsafe{ sip::clear_ssoft()};
    //TLB Invalidation IPI

    // Reschedule Request IPI

    // Function Call IPI

}

/// Interrupt handler
#[no_mangle]
extern "C" fn trap_handler_interrupt(f: &mut TrapFrame) {
    threadlet_syn_print(5, 102);
    let cause = riscv::register::scause::read().cause();
    match cause {
        Trap::Interrupt(interrupt) => {
            IS_KERNEL_INTERRUPTED.store(true);
            match interrupt {
                Interrupt::SupervisorTimer => call_irq_callback_functions(f, TIMER_IRQ_LINE),
                Interrupt::SupervisorExternal => handle_external_interrupts(f),
                Interrupt::SupervisorSoft => handle_software_interrupts(f),
                _ => panic!("Unsupported interrupt!"),
            }
            IS_KERNEL_INTERRUPTED.store(false);
        }
        _ => panic!("trap_handler_interrupt: not an interrupt"),
    }
}


/// Threadlet Interrupt handler 
#[no_mangle]
extern "C" fn trap_handler_threadlet_interrupt(f: &mut TrapFrame) {
    threadlet_syn_print(5, 102);
    let cause = riscv::register::scause::read().cause();
    match cause {
        Trap::Interrupt(interrupt) => {
            IS_KERNEL_INTERRUPTED.store(true);
            match interrupt {
                Interrupt::SupervisorTimer => call_irq_callback_functions_threadlet(f, TIMER_IRQ_LINE),
                Interrupt::SupervisorExternal => handle_external_interrupts_threadlet(f),
                Interrupt::SupervisorSoft => handle_software_interrupts(f),
                _ => panic!("Unsupported interrupt!"),
            }
            IS_KERNEL_INTERRUPTED.store(false);
        }
        _ => panic!("trap_handler_interrupt: not an interrupt"),
    }
}

/// Exception handler (vectored entry at BASE).
#[no_mangle]
extern "C" fn trap_handler_exception(f: &mut TrapFrame) {
    match riscv::register::scause::read().cause() {
        Trap::Exception(e) => {
            let stval = riscv::register::stval::read();
            // Determine the privilege level prior to trap via SPP (bit 8) in saved sstatus.
            // SPP == 0: came from U-mode (user); SPP == 1: came from S-mode (kernel).
            let from_user = (f.sstatus & (1 << 8)) == 0;
            if from_user {
                // Trap originates from user mode: let user-space VM handle the fault.
                handle_user_page_fault(f, stval, e);
                return;
            }

            // Kernel-mode exception. If the faulting address is in user range and
            // we have a current task, this is likely a kernel copy-to/from-user path.
            // Delegate to user page fault handler similarly to the user-origin case.
            if stval < MAX_USERSPACE_VADDR {
                if crate::task::Task::current().is_some() {
                    handle_user_page_fault(f, stval, e);
                    return;
                }
            }

            // Try exception table recovery for in-kernel faults (e.g., copy_*_user fixups).
            if let Some(addr) = ExTable::find_recovery_inst_addr(f.sepc) {
                f.sepc = addr;
                return;
            }

            // No recovery path; fatal kernel exception.
            panic!(
                "Cannot handle kernel cpu exception: {e:?}. stval: {stval:#x}, trapframe: {f:#x?}.",
            );
        }
        _ => panic!("trap_handler_exception: not an exception"),
    }
}

/// Handles page fault from user space.
fn handle_user_page_fault(f: &mut TrapFrame, page_fault_addr: usize, e: Exception) {
    let current_task = Task::current().unwrap();
    let user_space = current_task
        .user_space()
        .expect("the user space is missing when a page fault from the user happens.");

    let info = CpuExceptionInfo {
        code: e,
        page_fault_addr,
        error_code: 0,
    };

    let res = user_space.vm_space().handle_page_fault(&info);
    // Copying bytes by bytes can recover directly
    // if handling the page fault successfully.
    if res.is_ok() {
        return;
    }

    // Use the exception table to recover to normal execution.
    if let Some(addr) = ExTable::find_recovery_inst_addr(f.sepc) {
        f.sepc = addr;
    } else {
        panic!("Cannot handle user page fault; Trapframe: {:#x?}.", f);
    }
}
