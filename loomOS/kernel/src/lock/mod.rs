// SPDX-License-Identifier: MPL-2.0

//! Kernel-local locking primitives.

pub mod mcslock;
pub mod mcs_threadlet;

pub use mcslock::{McsGuard, McsLock, McsNode};

