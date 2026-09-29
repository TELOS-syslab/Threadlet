// SPDX-License-Identifier: MPL-2.0 OR MIT
//
// The original source code is from [trapframe-rs](https://github.com/rcore-os/trapframe-rs),
// which is released under the following license:
//
// SPDX-License-Identifier: MIT
//
// Copyright (c) 2020 - 2024 Runji Wang
//
// We make the following new changes:
// * Implement the `trap_handler` of Asterinas.
//
// These changes are released under the following license:
//
// SPDX-License-Identifier: MPL-2.0

extern crate alloc;

use alloc::collections::BTreeMap;
use core::arch::{asm, global_asm};

use crate::{
    arch::{boot::smp, riscv::threadlet},
    cpu,
    early_println,
    sync::SpinLock,
    task::KernelStack,
    Pod,
};
use spin::Once;

#[cfg(target_arch = "riscv32")]
global_asm!(
    r"
    .equ XLENB, 4
    .macro LOAD_SP a1, a2
        lw \a1, \a2*XLENB(sp)
    .endm
    .macro STORE_SP a1, a2
        sw \a1, \a2*XLENB(sp)
    .endm
"
);
#[cfg(target_arch = "riscv64")]
global_asm!(
    r"
    .equ XLENB, 8
    .macro LOAD_SP a1, a2
        ld \a1, \a2*XLENB(sp)
    .endm
    .macro STORE_SP a1, a2
        sd \a1, \a2*XLENB(sp)
    .endm
"
);

global_asm!(include_str!("trap.S"));
global_asm!(include_str!("trap_threadlet.S"));

#[derive(Debug)]
struct InterruptThreadletStack {
    stack: KernelStack,
    sp_top: usize,
    intr_hart_id: u32,
}

static INTR_THREADLET_STACKS: Once<SpinLock<BTreeMap<u32, InterruptThreadletStack>>> = Once::new();

fn stack_store() -> &'static SpinLock<BTreeMap<u32, InterruptThreadletStack>> {
    INTR_THREADLET_STACKS.call_once(|| SpinLock::new(BTreeMap::new()));
    INTR_THREADLET_STACKS.get().unwrap()
}

fn ensure_intr_threadlet_stack(hartid: u32, intr_hart_id: u32) -> Option<usize> {
    let mut guard = stack_store().lock();
    if let Some(existing) = guard.get(&hartid) {
        return Some(existing.sp_top);
    }

    let stack = KernelStack::new_with_guard_page().ok()?;
    let sp_top = stack.end_vaddr() - 16;

    guard.insert(
        hartid,
        InterruptThreadletStack {
            stack,
            sp_top,
            intr_hart_id,
        },
    );
    Some(sp_top)
}

fn init_interrupt_threadlet_context(hartid: u32, intr_hart_id: u32) -> bool {
    early_println!("[trap/threadlet] threadlet_intr found");
    threadlet::threadlet_init();
    let Some(sp_top) = ensure_intr_threadlet_stack(hartid, intr_hart_id) else {
        early_println!(
            "[trap/threadlet] failed to allocate stack for hart {} intr_hart {}",
            hartid,
            intr_hart_id
        );
        return false;
    };
    threadlet::threadlet_set_priority(intr_hart_id, 31);
    let gp = threadlet::read_current_gp();
    let tp = threadlet::read_current_tp();

    threadlet::threadlet_set_sp(intr_hart_id, sp_top);
    threadlet::threadlet_set_gp(intr_hart_id, gp);
    threadlet::threadlet_set_tp(intr_hart_id, tp);

    early_println!(
        "[trap/threadlet] hart {} intr_threadlet {} stack_top=0x{:x} gp=0x{:x} tp=0x{:x}",
        hartid,
        intr_hart_id,
        sp_top,
        gp,
        tp
    );
    true
}

fn use_intr_threadlet_vector(hartid: u32) -> Option<u32> {
    if !smp::intr_threadlet_present() {
        return None;
    }
    smp::intr_threadlet_for_hart(hartid)
}

/// Initialize interrupt handling for the current HART.
///
/// # Safety
///
/// This function will:
/// - Set `sscratch` to 0.
/// - Set `stvec` to internal exception vector.
///
/// You **MUST NOT** modify these registers later.
pub unsafe fn init(on_bsp: bool) {
    let _ = smp::get_num_processors();
    let hartid = if on_bsp {
        crate::arch::boot::BOOT_HART_ID.get().copied().unwrap_or(0) as u32
    } else {
        let cpu_id = cpu::current_cpu_id().map(|c| c.as_usize()).unwrap_or(0);
        smp::HART_ID_LIST
            .get()
            .and_then(|v| v.get(cpu_id).copied())
            .unwrap_or(0)
    };
    let intr_threadlet_target = use_intr_threadlet_vector(hartid).and_then(|intr_id| {
            if init_interrupt_threadlet_context(hartid as u32, intr_id) {
                Some(intr_id)
            } else {
                None
            }
        });

    // Set sscratch register to 0, indicating to exception/interrupt vector that we are
    // presently executing in the kernel
    asm!("csrw sscratch, zero");
    // Set the trap vector base in direct mode (lsb=0) so interrupts/exceptions share entry
    let base= trap_vector_base as usize;

    asm!("csrw stvec, {}", in(reg) base);
    early_println!(
        "[ostd::arch::riscv::trap::init] cpu={} set stvec=0x{:x}",
        hartid,
        base
    );
}

/// Trap frame of kernel interrupt
///
/// # Trap handler
///
/// You need to define a handler function like this:
///
/// ```no_run
/// #[no_mangle]
/// pub extern "C" fn trap_handler(tf: &mut TrapFrame) {
///     println!("TRAP! tf: {:#x?}", tf);
/// }
/// ```
#[derive(Debug, Default, Clone, Copy)]
#[repr(C)]
pub struct TrapFrame {
    /// General registers
    pub general: GeneralRegs,
    /// Supervisor Status
    pub sstatus: usize,
    /// Supervisor Exception Program Counter
    pub sepc: usize,
}

/// Saved registers on a trap.
#[derive(Debug, Default, Clone, Copy, Pod)]
#[repr(C)]
pub struct UserContext {
    /// General registers
    pub general: GeneralRegs,
    /// Supervisor Status
    pub sstatus: usize,
    /// Supervisor Exception Program Counter
    pub sepc: usize,
}

impl UserContext {
    /// Go to user space with the context, and come back when a trap occurs.
    ///
    /// On return, the context will be reset to the status before the trap.
    /// Trap reason and error code will be returned.
    ///
    /// # Example
    /// ```no_run
    /// use trapframe::{UserContext, GeneralRegs};
    ///
    /// // init user space context
    /// let mut context = UserContext {
    ///     general: GeneralRegs {
    ///         sp: 0x10000,
    ///         ..Default::default()
    ///     },
    ///     sepc: 0x1000,
    ///     ..Default::default()
    /// };
    /// // go to user
    /// context.run();
    /// // back from user
    /// println!("back from user: {:#x?}", context);
    /// ```
    pub fn run(&mut self) {
        unsafe { run_user(self) }
    }
}

/// General registers
#[derive(Debug, Default, Clone, Copy, Pod)]
#[repr(C)]
#[allow(missing_docs)]
pub struct GeneralRegs {
    pub zero: usize,
    pub ra: usize,
    pub sp: usize,
    pub gp: usize,
    pub tp: usize,
    pub t0: usize,
    pub t1: usize,
    pub t2: usize,
    pub s0: usize,
    pub s1: usize,
    pub a0: usize,
    pub a1: usize,
    pub a2: usize,
    pub a3: usize,
    pub a4: usize,
    pub a5: usize,
    pub a6: usize,
    pub a7: usize,
    pub s2: usize,
    pub s3: usize,
    pub s4: usize,
    pub s5: usize,
    pub s6: usize,
    pub s7: usize,
    pub s8: usize,
    pub s9: usize,
    pub s10: usize,
    pub s11: usize,
    pub t3: usize,
    pub t4: usize,
    pub t5: usize,
    pub t6: usize,
}

impl UserContext {
    /// Get number of syscall
    pub fn get_syscall_num(&self) -> usize {
        self.general.a7
    }

    /// Get return value of syscall
    pub fn get_syscall_ret(&self) -> usize {
        self.general.a0
    }

    /// Set return value of syscall
    pub fn set_syscall_ret(&mut self, ret: usize) {
        self.general.a0 = ret;
    }

    /// Get syscall args
    pub fn get_syscall_args(&self) -> [usize; 6] {
        [
            self.general.a0,
            self.general.a1,
            self.general.a2,
            self.general.a3,
            self.general.a4,
            self.general.a5,
        ]
    }

    /// Set instruction pointer
    pub fn set_ip(&mut self, ip: usize) {
        self.sepc = ip;
    }

    /// Set stack pointer
    pub fn set_sp(&mut self, sp: usize) {
        self.general.sp = sp;
    }

    /// Get stack pointer
    pub fn get_sp(&self) -> usize {
        self.general.sp
    }

    /// Set tls pointer
    pub fn set_tls(&mut self, tls: usize) {
        self.general.gp = tls;
    }
}

#[allow(improper_ctypes)]
extern "C" {
    pub fn trap_vector_base();
    pub fn trap_threadlet_vector_base();
    pub fn run_user(regs: &mut UserContext);
}
