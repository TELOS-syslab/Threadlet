// SPDX-License-Identifier: MPL-2.0

//! Probe icenet-like MMIO devices from FDT and print basic info.
























// 0xF0（R/W，1+nCores bit；bit0=TX，bit(1+i)=RX_i）



















// RX_i ISR：












use alloc::vec::Vec;
use crate::cpu::CpuId;
use fdt::node::FdtNode;
use crate::{
    arch::{boot::DEVICE_TREE, device::plic::{enable_external_interrupt, enable_external_interrupt_on}},
    cpu::PinCurrentCpu,
    io_mem::IoMem,
    mm::{VmIoOnce, CachePolicy, PageFlags},
    trap::{IrqLine, TrapFrame},
};
use spin::Once;

static IRQ_HOOK: Once<fn(usize)> = Once::new();

static ICENET_IRQ_LINES: Once<Vec<IrqLine>> = Once::new();
static ICENET_MMIO: Once<IoMem> = Once::new();

pub(crate) fn probe_fdt() {
    let fdt = match DEVICE_TREE.get() {
        Some(v) => v,
        None => return,
    };

    // Helper: robustly parse interrupts from a node.
    // 1) 'interrupts' as be32 array
    // 2) 'interrupts-extended' as pairs (phandle, spec), prefer PLIC phandle if present
    fn parse_irqs(node: &FdtNode<'_, '_>) -> alloc::vec::Vec<u32> {
        // Try 'interrupts'
        if let Some(p) = node.property("interrupts") {
            let bytes = p.value;
            let mut out = alloc::vec::Vec::new();
            let mut i = 0usize;
            while i + 4 <= bytes.len() {
                let v = u32::from_be_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
                out.push(v);
                i += 4;
            }
            if !out.is_empty() { return out; }
        }

        // Resolve PLIC phandle for better matching
        let fdt = DEVICE_TREE.get().unwrap();
        let plic_node = fdt
            .find_node("/soc/plic")
            .or_else(|| {
                fdt.find_node("/soc").and_then(|soc| {
                    soc.children().find(|n| n.compatible().is_some_and(|c| {
                        c.all().any(|s| s == "riscv,plic0" || s == "sifive,plic-1.0.0")
                    }))
                })
            });
        let plic_phandle: Option<u32> = plic_node
            .and_then(|n| n.property("phandle").map(|p| p.as_usize().unwrap_or(0) as u32))
            .filter(|&h| h != 0);

        // Try 'interrupts-extended'
        if let Some(p) = node.property("interrupts-extended") {
            let bytes = p.value;
            let mut out = alloc::vec::Vec::new();
            let mut i = 0usize;
            while i + 8 <= bytes.len() {
                let ph = u32::from_be_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
                let spec = u32::from_be_bytes([
                    bytes[i + 4], bytes[i + 5], bytes[i + 6], bytes[i + 7],
                ]);
                if plic_phandle.map_or(true, |h| h == ph) {
                    out.push(spec);
                }
                i += 8;
            }
            if !out.is_empty() { return out; }
        }
        alloc::vec::Vec::new()
    }

    // Try both explicit path and a generic scan under /soc for nodes named ice-nic@
    let mut found_any = false;
    for node in fdt.find_all_nodes("/soc/ice-nic") {
        if let Some(reg) = node.reg() {
            if let Some(r) = reg.into_iter().next() {
                let base = r.starting_address as usize;
                let size = r.size.unwrap_or(0) as usize;
                // Read interrupts from DT robustly
                let ints: alloc::vec::Vec<u32> = parse_irqs(&node);
                crate::early_print!(
                    "[icenet-probe] node='{}' base=0x{:x} size=0x{:x} irqs_from_dt={:?}\n",
                    node.name,
                    base,
                    size,
                    ints
                );
                // Prepare MMIO mapping for later software trigger
                if ICENET_MMIO.get().is_none() {
                    // SAFETY: map icenet device MMIO with A/D preset to avoid Rocket first-access faults.
                    // Use the common helper for RISC-V device mappings to keep flags consistent with PLIC.
                    let io = unsafe { super::create_device_io_mem(base as *const u8, size) };
                    ICENET_MMIO.call_once(|| io);
                    crate::early_println!("[icenet-reg] mmio mapped at base=0x{:x} len=0x{:x}", base, size);
                }



                // Register per-queue IRQ callbacks (TX + per-core RX)
                if !ints.is_empty() {
                    let ints_owned = ints.clone();
                    // If DT provides exactly per-core RX IRQs (no TX), start queue index from 1.
                    let idx_offset: usize = if ints_owned.len() <= 2 { 1 } else { 0 };
                    crate::early_println!(
                        "[icenet-reg] registering {} IRQs from DT for node '{}' (idx_offset={})",
                        ints_owned.len(), node.name, idx_offset
                    );
                    ICENET_IRQ_LINES.call_once(move || {
                        let mut lines = Vec::new();
                        for (j, irq_ref) in ints_owned.iter().enumerate() {
                            let irq_num = *irq_ref as u32;
                            let idx = j + idx_offset; // idx==0 reserved for TX if present
                            crate::early_println!(
                                "[icenet-reg] enabling irq={} for q={}",
                                irq_num, idx
                            );
                            // Map: idx==2 => RX core1 on CPU1 (two-core config); others on CPU0
                            if idx == 2 {
                                if let Ok(cpu1) = CpuId::try_from(1usize) {
                                    enable_external_interrupt_on(cpu1, irq_num as u16);
                                } else {
                                    enable_external_interrupt(irq_num as u16);
                                }
                            } else {
                                enable_external_interrupt(irq_num as u16);
                            }
                            if let Ok(mut line) = IrqLine::alloc_specific(irq_num as u8) {
                                let q = idx;
                                line.on_active(move |_tf: &TrapFrame| {
                                    if cfg!(irqdebug) {
                                        let guard = crate::trap::disable_local();
                                        let cpu = guard.current_cpu().as_usize();
                                        crate::early_println!(
                                            "[icenet-irq] cpu={} irq={} q={}",
                                            cpu, irq_num, q
                                        );
                                    }
                                    if let Some(hook) = IRQ_HOOK.get() { hook(q); }
                                });
                                lines.push(line);
                            }
                        }
                        lines
                    });
                }
                found_any = true;
            }
        }
    }

    // Some platforms may name by compatible only; do a broader scan under /soc
    // and filter by compatible string "ucb-bar,ice-nic".
    if !found_any {
        if let Some(soc) = fdt.find_node("/soc") {
            for child in soc.children() {
                if child
                    .compatible()
                    .is_some_and(|c| c.all().any(|s| s == "ucb-bar,ice-nic"))
                {
                    if let Some(reg) = child.reg() {
                        if let Some(r) = reg.into_iter().next() {
                            let base = r.starting_address as usize;
                            let size = r.size.unwrap_or(0) as usize;
                            // Read interrupts from DT robustly for compat match
                            let ints: alloc::vec::Vec<u32> = parse_irqs(&child);
                            crate::early_print!(
                                "[icenet-probe] compat='ucb-bar,ice-nic' base=0x{:x} size=0x{:x} irqs_from_dt={:?}\n",
                                base,
                                size,
                                ints
                            );
                            if !ints.is_empty() && ICENET_IRQ_LINES.get().is_none() {
                                if ICENET_MMIO.get().is_none() {
                                    // SAFETY: map icenet device MMIO with A/D preset 
                                    // Use the common helper for RISC-V device mappings to keep flags consistent with PLIC.
                                    let io = unsafe { super::create_device_io_mem(base as *const u8, size) };
                                    ICENET_MMIO.call_once(|| io);
                                    crate::early_println!("[icenet-reg] mmio mapped at base=0x{:x} len=0x{:x}", base, size);
                                }

                                let ints_owned = ints.clone();
                                let idx_offset: usize = if ints_owned.len() <= 2 { 1 } else { 0 };
                                crate::early_println!(
                                    "[icenet-reg] registering {} IRQs from DT for compat 'ucb-bar,ice-nic' (idx_offset={})",
                                    ints_owned.len(), idx_offset
                                );
                                ICENET_IRQ_LINES.call_once(move || {
                                    let mut lines = Vec::new();
                                    for (j, irq_ref) in ints_owned.iter().enumerate() {
                                        let irq_num = *irq_ref as u32;
                                        let idx = j + idx_offset;
                                        crate::early_println!(
                                            "[icenet-reg] enabling irq={} for q={}",
                                            irq_num, idx
                                        );
                                        // IRQ affinity policy:
                                        // - idx==0: TX line -> keep on BSP (CPU0)
                                        // - idx==1/2: RX queues -> bind to CPU1 as requested
                                        //   (do NOT additionally enable on CPU0 to avoid migration)
                                        if idx == 2 {
                                            if let Ok(cpu1) = CpuId::try_from(1usize) {
                                                enable_external_interrupt_on(cpu1, irq_num as u16);
                                            } else {
                                                // Fallback: if CPU1 is not available, enable on BSP
                                                enable_external_interrupt(irq_num as u16);
                                            }
                                        } else {
                                            // Default: enable on BSP
                                            enable_external_interrupt(irq_num as u16);
                                        }
                                        if let Ok(mut line) = IrqLine::alloc_specific(irq_num as u8) {
                                            let q = idx;
                                            line.on_active(move |_tf: &TrapFrame| {
                                                let guard = crate::trap::disable_local();
                                                let cpu = guard.current_cpu().as_usize();
                                                crate::early_println!(
                                                    "[icenet-irq] cpu={} irq={} q={}",
                                                    cpu, irq_num, q
                                                );
                                                if let Some(hook) = IRQ_HOOK.get() { hook(q); }
                                            });
                                            crate::early_println!(
                                                "[icenet-reg] registered callback irq={} q={}",
                                                irq_num, idx
                                            );
                                            lines.push(line);
                                        }
                                    }
                                    lines
                                });
                            }
                            found_any = true;
                        }
                    }
                }
            }
        }
    }

    if !found_any {
        // Optional: single line to indicate no icenet node.
        crate::early_println!("[icenet-probe] no ice-nic node found in FDT");
    }
}

/// Software-trigger icenet IRQs for validation (Phase 3a).
/// This can be called after the OS fully boots to verify each queue's IRQ wiring.
pub fn trigger_test_irqs() {
    let Some(io) = ICENET_MMIO.get() else {
        return;
    };
    crate::early_println!("[icenet-test] Start trigger_test_irqs");
    // Enable NIC int_mask (RX only; TX disabled)
    let _ = io.write_once::<u32>(0xF0, &0x0000_0006u32);
    // 0xFC: DEBUG_TRIG bits: bit0 RX core0, bit1 RX core1, bit8 TX

    let _ = io.write_once::<u32>(0xFC, &0x0000_0001u32); // RX core0
    // small delay to avoid mixed prints from two cores
    for _ in 0..100_0000 {
        core::hint::spin_loop();
    }
    let _ = io.write_once::<u32>(0xFC, &0x0000_0002u32); // RX core1
    for _ in 0..100_0000 {
        core::hint::spin_loop();
    }
    let _ = io.write_once::<u32>(0xFC, &0x0000_0002u32); // RX core1 again
    for _ in 0..100_0000 {
        core::hint::spin_loop();
    }
    crate::early_println!("[icenet-test] debug-triggered comps: RX0, RX1");
    // Dump PLIC state for CPU0 and the two IRQs
    crate::arch::riscv::device::plic::debug_dump_irq_on(CpuId::bsp(), 12);
    crate::arch::riscv::device::plic::debug_dump_irq_on(CpuId::bsp(), 13);
    if let Ok(cpu1) = CpuId::try_from(1usize) {
        crate::arch::riscv::device::plic::debug_dump_irq_on(cpu1, 14);
    }
}
/// Software-trigger a TX submit to test send path (Phase 1).
/// Writes a dummy descriptor to 0x00 (addr=0, len=60, partial=0).


/// Returns a clone of IoMem for icenet MMIO if present.
pub fn get_mmio() -> Option<IoMem> { ICENET_MMIO.get().cloned() }

/// Sets an IRQ hook to be called in hardirq context when an icenet IRQ fires.
pub fn set_irq_hook(hook: fn(usize)) { let _ = IRQ_HOOK.call_once(|| hook); }
