// SPDX-License-Identifier: MPL-2.0

//! Dummy RTC driver for platforms without a discoverable RTC device.
//!
//! On some RISC-V setups (e.g., when passing an external DTB without `/soc/rtc`),
//! there is no Goldfish RTC described. This fallback driver allows the time
//! component to initialize so that the rest of the system can boot. It reports
//! UNIX epoch as the current time.

use crate::{SystemTime, rtc::Driver};

pub struct RtcDummy;

impl Driver for RtcDummy {
    fn try_new() -> Option<RtcDummy> {
        Some(RtcDummy)
    }

    fn read_rtc(&self) -> SystemTime {
        // Return a valid date/time: 1970-01-01 00:00:00.000000000
        SystemTime {
            year: 1970,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
            nanos: 0,
        }
    }
}

