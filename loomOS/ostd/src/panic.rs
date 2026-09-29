// SPDX-License-Identifier: MPL-2.0

//! Panic support.

use core::ffi::c_void;

pub use unwinding::panic::{begin_panic, catch_unwind};

use crate::{
    arch::qemu::{exit_qemu, QemuExitCode},
    early_print, early_println,
    sync::SpinLock,
};

extern crate cfg_if;
extern crate gimli;

use gimli::Register;
use unwinding::abi::{
    UnwindContext, UnwindReasonCode, _Unwind_Backtrace, _Unwind_FindEnclosingFunction,
    _Unwind_GetGR, _Unwind_GetIP,
};

/// The default panic handler for OSTD based kernels.
///
/// The user can override it by defining their own panic handler with the macro
/// `#[ostd::panic_handler]`.
#[linkage = "weak"]
#[no_mangle]
pub fn __ostd_panic_handler(info: &core::panic::PanicInfo) -> ! {
    let _irq_guard = crate::trap::disable_local();

    crate::cpu_local_cell! {
        static IN_PANIC: bool = false;
    }

    if IN_PANIC.load() {
        early_println!("The panic handler panicked {:#?}", info);
        abort();
    }

    IN_PANIC.store(true);

    early_println!("Non-resettable panic! {:#?}", info);

    print_stack_trace();
    abort();
}

/// Abort handler on RISC-V: never reset on FPGA; park forever instead.
///
/// We intentionally avoid calling `system_reset` here to prevent real hardware
/// (FPGA) from resetting immediately, which makes logs hard to capture. On
/// panics or fatal paths we just halt in a low-power loop.
pub fn abort() -> ! {
    #[cfg(target_arch = "riscv64")]
    {
        loop {
            unsafe { riscv::asm::wfi() }
        }
    }
    #[cfg(not(target_arch = "riscv64"))]
    {
        exit_qemu(QemuExitCode::Failed);
    }
}

/// Prints the stack trace of the current thread to the console.
///
/// The printing procedure is protected by a spin lock to prevent interleaving.
pub fn print_stack_trace() {
    /// We acquire a global lock to prevent the frames in the stack trace from
    /// interleaving. The spin lock is used merely for its simplicity.
    static BACKTRACE_PRINT_LOCK: SpinLock<()> = SpinLock::new(());
    let _lock = BACKTRACE_PRINT_LOCK.lock();

    early_println!("Printing stack trace:");

    struct CallbackData {
        counter: usize,
    }
    extern "C" fn callback(unwind_ctx: &UnwindContext<'_>, arg: *mut c_void) -> UnwindReasonCode {
        let data = unsafe { &mut *(arg as *mut CallbackData) };
        data.counter += 1;
        let pc = _Unwind_GetIP(unwind_ctx);
        if pc > 0 {
            let fde_initial_address = _Unwind_FindEnclosingFunction(pc as *mut c_void) as usize;
            early_println!(
                "{:4}: fn {:#18x} - pc {:#18x} / registers:",
                data.counter,
                fde_initial_address,
                pc,
            );
        }
        // Print the first 8 general registers for any architecture. The register number follows
        // the DWARF standard.
        for i in 0..8u16 {
            let reg_i = _Unwind_GetGR(unwind_ctx, i as i32);
            cfg_if::cfg_if! {
                if #[cfg(target_arch = "x86_64")] {
                    let reg_name = gimli::X86_64::register_name(Register(i)).unwrap_or("unknown");
                } else if #[cfg(target_arch = "riscv64")] {
                    let reg_name = gimli::RiscV::register_name(Register(i)).unwrap_or("unknown");
                } else if #[cfg(target_arch = "aarch64")] {
                    let reg_name = gimli::AArch64::register_name(Register(i)).unwrap_or("unknown");
                } else {
                    let reg_name = "unknown";
                }
            }
            if i % 4 == 0 {
                early_print!("\n    ");
            }
            early_print!(" {} {:#18x};", reg_name, reg_i);
        }
        early_print!("\n\n");
        UnwindReasonCode::NO_REASON
    }

    let mut data = CallbackData { counter: 0 };
    _Unwind_Backtrace(callback, &mut data as *mut _ as _);

    #[cfg(target_arch = "riscv64")]
    if data.counter == 0 {
        early_println!("No DWARF frames. Falling back to FP walk (riscv64)");
        unsafe { fallback_backtrace_riscv_fp(); }
    }
}

#[cfg(target_arch = "riscv64")]
unsafe fn fallback_backtrace_riscv_fp() {
    use core::arch::asm;
    use core::mem::size_of;

    // Best-effort: walk s0 (frame pointer) chain.
    // Rationale:
    //  - With `-C force-frame-pointers=yes`, rustc uses s0 as FP.
    //  - Common prologue:
    //      addi sp, sp, -N
    //      sd   ra, N-8(sp)
    //      sd   s0, N-16(sp)
    //      addi s0, sp, N     ; s0 points to top of frame
    //    Thus at [s0-8] is saved RA, at [s0-16] is previous FP.

    const MAX_FRAMES: usize = 128;

    let mut fp: usize;
    asm!("mv {out}, s0", out = out(reg) fp, options(nomem, nostack, preserves_flags));

    if fp == 0 {
        early_println!("[fpbt] current FP is zero; abort fallback");
        return;
    }

    early_println!("[fpbt] start fp = {:#018x}", fp);

    #[inline]
    unsafe fn try_copy<T>(dst: &mut T, src: *const T) -> bool {
        extern "C" {
            fn __memcpy_fallible(dst: *mut u8, src: *const u8, size: usize) -> usize;
        }
        let failed = __memcpy_fallible(
            (dst as *mut T).cast::<u8>(),
            src.cast::<u8>(),
            size_of::<T>(),
        );
        failed == 0
    }

    for idx in 1..=MAX_FRAMES {
        // Read saved RA at [fp - 8] and previous FP at [fp - 16].
        let ra_ptr = (fp as *const u8).wrapping_sub(8) as *const usize;
        let pfp_ptr = (fp as *const u8).wrapping_sub(16) as *const usize;
        let mut ra: usize = 0;
        let mut prev_fp: usize = 0;

        if !try_copy(&mut ra, ra_ptr) || !try_copy(&mut prev_fp, pfp_ptr) {
            early_println!("[fpbt] memory read fault; stop");
            break;
        }

        if ra == 0 || prev_fp == 0 {
            early_println!("[fpbt] reached null ra/fp; stop");
            break;
        }

        // Try to resolve enclosing function using unwind tables if present.
        let fde_initial_address = _Unwind_FindEnclosingFunction(ra as *mut c_void) as usize;
        if fde_initial_address != 0 {
            early_println!(
                "{:4}: fn {:#018x} - pc {:#018x}  fp {:#018x}",
                idx, fde_initial_address, ra, fp
            );
        } else {
            early_println!(
                "{:4}: fn {:#018x} - pc {:#018x}  fp {:#018x}",
                idx, 0usize, ra, fp
            );
        }

        if prev_fp == fp || (prev_fp & 0xF) != 0 {
            early_println!("[fpbt] suspect fp chain (prev={:#x}); stop", prev_fp);
            break;
        }

        fp = prev_fp;
    }
}
