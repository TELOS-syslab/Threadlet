// SPDX-License-Identifier: MPL-2.0

pub mod priority;

mod priority_scheduler;
mod sched_class;
mod stats;

// Export the stats getter functions.
pub use stats::{loadavg, nr_queued_and_running};


pub use self::priority_scheduler::init;
// There may be multiple scheduling policies in the system,
// and subsequent schedulers can be placed under this module.
pub use self::sched_class::SchedAttr;
