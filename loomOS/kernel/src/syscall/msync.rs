// SPDX-License-Identifier: MPL-2.0

use super::SyscallReturn;
use crate::prelude::*;

pub fn sys_msync(_start: Vaddr, _size: usize, _flag: i32, _ctx: &Context) -> Result<SyscallReturn> {

    Ok(SyscallReturn::Return(0))
}
