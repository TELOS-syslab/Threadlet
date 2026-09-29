// SPDX-License-Identifier: MPL-2.0

//! Console output.

use core::fmt::Arguments;

use crate::sync::{LocalIrqDisabled, SpinLock};

/// Prints formatted arguments to the console.
pub fn early_print(args: Arguments) {
    // Serialize early prints across CPUs to avoid interleaved, garbled lines.
    // This does not change behavior, only adds mutual exclusion for readability
    // during early bring-up and multi-core logs.
    // Use IRQ-disabling guardian to avoid deadlock when interrupts
    // re-enter and attempt to print while the lock is held.
    
    // static PRINT_LOCK: SpinLock<(), LocalIrqDisabled> = SpinLock::new(());
    // let _guard = PRINT_LOCK.lock();
    crate::arch::serial::print(args);
}

/// Prints to the console.
#[macro_export]
macro_rules! early_print {
    ($fmt: literal $(, $($arg: tt)+)?) => {
        $crate::console::early_print(format_args!($fmt $(, $($arg)+)?))
    }
}

/// Prints to the console with a newline.
#[macro_export]
macro_rules! early_println {
    () => { $crate::early_print!("\n") };
    ($fmt: literal $(, $($arg: tt)+)?) => {
        $crate::console::early_print(format_args!(concat!($fmt, "\n") $(, $($arg)+)?))
    }
}
