// SPDX-License-Identifier: MPL-2.0

//! Handles trap across kernel and user space.

mod handler;
mod irq;

pub use handler::{in_interrupt_context, register_bottom_half_handler};
pub use handler::register_bottom_half_handler_threadlet;

pub(crate) use self::handler::{
    call_irq_callback_functions,
    call_irq_callback_functions_threadlet,
};
pub use self::irq::{disable_local, DisabledLocalIrqGuard, IrqCallbackFunction, IrqLine};
pub use crate::arch::trap::TrapFrame;
