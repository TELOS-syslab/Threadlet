// SPDX-License-Identifier: MPL-2.0

//! MSI-X capability support.

#![allow(dead_code)]
#![allow(unused_variables)]

use alloc::{sync::Arc, vec::Vec};

use cfg_if::cfg_if;

use crate::{
    arch::iommu::has_interrupt_remapping,
    bus::pci::{
        cfg_space::{Bar, Command, MemoryBar},
        common_device::PciCommonDevice,
        device_info::PciDeviceLocation,
    },
    mm::VmIoOnce,
    trap::IrqLine,
};

cfg_if! {
    if #[cfg(all(target_arch = "x86_64", feature = "cvm_guest"))] {
        use ::tdx_guest::tdx_is_enabled;
        use crate::arch::tdx_guest;
    }
}

/// MSI-X capability. It will set the BAR space it uses to be hidden.
#[derive(Debug)]
#[repr(C)]
pub struct CapabilityMsixData {
    loc: PciDeviceLocation,
    ptr: u16,
    table_size: u16,
    /// MSIX table entry content:
    /// | Vector Control: u32 | Msg Data: u32 | Msg Upper Addr: u32 | Msg Addr: u32 |
    table_bar: Arc<MemoryBar>,
    /// Pending bits table.
    pending_table_bar: Arc<MemoryBar>,
    table_offset: usize,
    pending_table_offset: usize,
    irqs: Vec<Option<IrqLine>>,
}

impl Clone for CapabilityMsixData {
    fn clone(&self) -> Self {
        let new_vec = self.irqs.clone().to_vec();
        Self {
            loc: self.loc,
            ptr: self.ptr,
            table_size: self.table_size,
            table_bar: self.table_bar.clone(),
            pending_table_bar: self.pending_table_bar.clone(),
            irqs: new_vec,
            table_offset: self.table_offset,
            pending_table_offset: self.pending_table_offset,
        }
    }
}

#[cfg(target_arch = "x86_64")]
const MSIX_DEFAULT_MSG_ADDR: u32 = 0xFEE0_0000;
#[cfg(target_arch = "riscv64")]
const MSIX_DEFAULT_MSG_ADDR: u32 = 0x0000_0000;

impl CapabilityMsixData {
    pub(super) fn new(dev: &mut PciCommonDevice, cap_ptr: u16) -> Self {
        todo!()
    }

    /// MSI-X Table size
    pub fn table_size(&self) -> u16 {
        // bit 10:0 table size
        (self.loc.read16(self.ptr + 2) & 0b11_1111_1111) + 1
    }

    /// Enables an interrupt line, it will replace the old handle with the new handle.
    pub fn set_interrupt_vector(&mut self, irq: IrqLine, index: u16) {
        if index >= self.table_size {
            return;
        }

        // If interrupt remapping is enabled, then we need to change the value of the message address.
        if has_interrupt_remapping() {
            let mut handle = irq.inner_irq().bind_remapping_entry().unwrap().lock();

            // Enable irt entry
            let irt_entry_mut = handle.irt_entry_mut().unwrap();
            irt_entry_mut.enable_default(irq.num() as u32);

            // Use remappable format. The bits[4:3] should be always set to 1 according to the manual.
            let mut address = MSIX_DEFAULT_MSG_ADDR | 0b1_1000;

            // Interrupt index[14:0] is on address[19:5] and interrupt index[15] is on address[2].
            address |= (handle.index() as u32 & 0x7FFF) << 5;
            address |= (handle.index() as u32 & 0x8000) >> 13;

            self.table_bar
                .io_mem()
                .write_once((16 * index) as usize + self.table_offset, &address)
                .unwrap();
            self.table_bar
                .io_mem()
                .write_once((16 * index + 8) as usize + self.table_offset, &0)
                .unwrap();
        } else {
            self.table_bar
                .io_mem()
                .write_once(
                    (16 * index + 8) as usize + self.table_offset,
                    &(irq.num() as u32),
                )
                .unwrap();
        }

        let _old_irq = core::mem::replace(&mut self.irqs[index as usize], Some(irq));
        // Enable this msix vector
        self.table_bar
            .io_mem()
            .write_once((16 * index + 12) as usize + self.table_offset, &0_u32)
            .unwrap();
    }

    /// Gets mutable IrqLine. User can register callbacks by using this function.
    pub fn irq_mut(&mut self, index: usize) -> Option<&mut IrqLine> {
        self.irqs[index].as_mut()
    }

    /// Returns true if MSI-X Enable bit is set.
    pub fn is_enabled(&self) -> bool {
        let msg_ctrl = self.loc.read16(self.ptr + 2);
        msg_ctrl & 0x8000 != 0
    }
}

fn set_bit(origin_value: u16, offset: usize, set: bool) -> u16 {
    (origin_value & (!(1 << offset))) | ((set as u16) << offset)
}
