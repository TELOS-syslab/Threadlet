// SPDX-License-Identifier: MPL-2.0

//! The standard library for Asterinas and other Rust OSes.
#![feature(alloc_error_handler)]
#![feature(allocator_api)]
#![feature(btree_cursors)]
#![feature(const_ptr_sub_ptr)]
#![feature(const_trait_impl)]
#![feature(core_intrinsics)]
#![feature(coroutines)]
#![feature(fn_traits)]
#![feature(generic_const_exprs)]
#![feature(iter_from_coroutine)]
#![feature(let_chains)]
#![feature(linkage)]
#![feature(min_specialization)]
#![feature(negative_impls)]
#![feature(ptr_metadata)]
#![feature(ptr_sub_ptr)]
#![feature(sync_unsafe_cell)]
#![feature(trait_upcasting)]
// The `generic_const_exprs` feature is incomplete however required for the page table
// const generic implementation. We are using this feature in a conservative manner.
#![allow(incomplete_features)]
#![allow(internal_features)]
#![no_std]
#![warn(missing_docs)]

extern crate alloc;
extern crate static_assertions;

pub mod arch;
pub mod boot;
pub mod bus;
pub mod collections;
pub mod console;
pub mod cpu;
mod error;
pub mod io_mem;
pub mod logger;
pub mod mm;
pub mod panic;
pub mod prelude;
pub mod smp;
pub mod sync;
pub mod task;
pub mod timer;
pub mod trap;
pub mod user;

use core::sync::atomic::{AtomicBool, Ordering};

pub use ostd_macros::{main, panic_handler};
pub use ostd_pod::Pod;

use crate::arch::{boot::smp::threadlet_present, threadlet::threadlet_print_disable};

pub use self::{error::Error, prelude::Result};

/// Initializes OSTD.
///
/// This function represents the first phase booting up the system. It makes
/// all functionalities of OSTD available after the call.
///
/// # Safety
///
/// This function should be called only once and only on the BSP.
//

// make inter-initialization-dependencies more clear and reduce usages of
// boot stage only global variables.
#[doc(hidden)]
unsafe fn init() {
    crate::early_println!("[ostd::init] enable_cpu_features.");
    arch::enable_cpu_features();
    crate::early_println!("[ostd::init] serial::init()");
    arch::serial::init();

    crate::early_println!("[ostd::init] early_init_bsp_local_base()");
    // SAFETY: This function is called only once and only on the BSP.
    unsafe { cpu::local::early_init_bsp_local_base() };

    crate::early_println!("[ostd::init] heap_allocator::init()");
    // SAFETY: This function is called only once and only on the BSP.
    unsafe { mm::heap_allocator::init() };
    crate::early_println!("[ostd::init] boot::init()");
    boot::init();
    crate::early_println!("[ostd::init] boot::init() returned");
    logger::init();

    crate::early_println!("[ostd::init] allocator::init()");
    mm::frame::allocator::init();
    crate::early_println!("[ostd::init] init_kernel_page_table");
    mm::kspace::init_kernel_page_table(mm::init_page_meta());
    crate::early_println!("[ostd::init] dma::init()");
    mm::dma::init();

    crate::early_println!("[ostd::init] init_on_bsp");
    arch::init_on_bsp();     // here we do smp::init();
    crate::early_println!("[ostd::init] init_on_bsp finished.");


    // SAFETY: This function is called only once on the BSP.
    unsafe {
        mm::kspace::activate_kernel_page_table();
    }

    crate::early_println!("[ostd::init] activate_kernel_page_table finished.");

    // Now the kernel page table is active; complete UART input hookup
    // after PLIC/IRQ allocator and VM mappings are ready.
    #[cfg(target_arch = "riscv64")]
    {
        arch::serial::late_enable();
    }

    crate::early_println!("[ostd::init] try bus init.");
    //here we do enable_external_interrupt()
    bus::init();

    
    if crate::arch::boot::smp::intr_threadlet_present() {
        threadlet_print_disable();
    }
    // From here we set sie and enable local interrupts on the BSP.
    arch::irq::enable_local();
    crate::early_println!("[ostd::init] finish irq::enable_local()");

    invoke_ffi_init_funcs();
    // crate::early_println!("[ostd::init] invoke_ffi_init_funcs() returned");

    IN_BOOTSTRAP_CONTEXT.store(false, Ordering::Relaxed);
    crate::early_println!("[ostd::init] finished");
}

/// Indicates whether the kernel is in bootstrap context.
pub(crate) static IN_BOOTSTRAP_CONTEXT: AtomicBool = AtomicBool::new(true);

/// Invoke the initialization functions defined in the FFI.
/// The component system uses this function to call the initialization functions of
/// the components.
fn invoke_ffi_init_funcs() {
    extern "C" {
        fn __sinit_array();
        fn __einit_array();
    }
    let call_len = (__einit_array as usize - __sinit_array as usize) / 8;
    for i in 0..call_len {
        unsafe {
            let function = (__sinit_array as usize + 8 * i) as *const fn();
            (*function)();
        }
    }
}
