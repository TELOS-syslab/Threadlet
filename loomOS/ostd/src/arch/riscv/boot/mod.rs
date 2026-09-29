// SPDX-License-Identifier: MPL-2.0

//! The RISC-V boot module defines the entrypoints of Asterinas.

#[cfg(target_arch = "riscv64")]
use rustsbi;

pub mod smp;

use alloc::{string::String, vec::Vec};
use core::{arch::global_asm};

use fdt::Fdt;
use spin::Once;

use crate::{
    boot::{
        kcmdline::KCmdlineArg,
        memory_region::{non_overlapping_regions_from, MemoryRegion, MemoryRegionType},
        BootloaderAcpiArg, BootloaderFramebufferArg,
    },
    early_println,
    mm::{self, paddr_to_vaddr},
};

global_asm!(include_str!("boot.S"));

/// The Flattened Device Tree of the platform.
pub static DEVICE_TREE: Once<Fdt> = Once::new();

/// The hardware hart ID of the boot hart (passed by firmware via a0).
///
/// Stored at the very beginning of `riscv_boot` so other modules (e.g., SMP
/// bring-up) can avoid re-starting the BSP via SBI HSM.
pub static BOOT_HART_ID: Once<usize> = Once::new();

fn init_bootloader_name(bootloader_name: &'static Once<String>) {
    early_println!("[ostd::init_bootloader_name] start");
    // Print key addresses to diagnose mapping:
    // - address of a known rodata literal
    // - section boundaries from linker
    // - satp/stvec snapshot
    let ro_unknown = "Unknown";
    early_println!(
        "[dbg] rodata literal 'Unknown' ptr=0x{:x}",
        ro_unknown.as_ptr() as usize
    );
    unsafe {
        extern "C" {
            static __kernel_start: u8;
            static __etext: u8;
            static __bss: u8;
            static __bss_end: u8;
            static __ex_table: u8;
            static __ex_table_end: u8;
            static __trampoline_start: u8;
            static __trampoline_end: u8;
        }
        early_println!(
            "[dbg] __kernel_start=0x{:x} __etext=0x{:x}",
            &__kernel_start as *const _ as usize,
            &__etext as *const _ as usize
        );
        early_println!(
            "[dbg] __bss=0x{:x} __bss_end=0x{:x}",
            &__bss as *const _ as usize,
            &__bss_end as *const _ as usize
        );
        early_println!(
            "[dbg] __ex_table=0x{:x} __ex_table_end=0x{:x}",
            &__ex_table as *const _ as usize,
            &__ex_table_end as *const _ as usize
        );
        early_println!(
            "[dbg] __trampoline_start=0x{:x} __trampoline_end=0x{:x}",
            &__trampoline_start as *const _ as usize,
            &__trampoline_end as *const _ as usize
        );
    }
    let satp_val = riscv::register::satp::read();
    let stvec_val = riscv::register::stvec::read();
    let stvec_bits = stvec_val.bits();
    early_println!(
        "[dbg] satp: mode={:?} asid=0x{:x} ppn=0x{:x}",
        satp_val.mode(),
        satp_val.asid(),
        satp_val.ppn()
    );
    early_println!("[dbg] stvec=0x{:x}", stvec_bits & !0x3usize);
    early_println!(
        "[dbg] BOOTLOADER_NAME Once @ 0x{:x}",
        bootloader_name as *const _ as usize
    );
    bootloader_name.call_once(|| "Unknown".into());
    early_println!("[ostd::init_bootloader_name] bootloader name finish");
}

fn init_kernel_commandline(kernel_cmdline: &'static Once<KCmdlineArg>) {
    early_println!("[ostd::init_kernel_commandline] start");

    // Prefer DT-provided bootargs; if missing/empty, fall back to a safe default
    // that includes an explicit init and a minimal environment.
    // This avoids panicking later when kernel expects a valid init path.
    let fdt = DEVICE_TREE.get().unwrap();
    let chosen_args_opt = fdt.chosen().bootargs();
    let args: &str = match chosen_args_opt {
        Some(s) if !s.trim().is_empty() => {
            early_println!("[kcmdline] using FDT bootargs: {}", s);
            s
        }
        _ => {
            // Fallback for FPGA: include static icenet addressing via module args.
            // Note: module arg format supports a single-level module.option key.
            // We use 'icenet.addr' and 'icenet.gw'.
            let fallback = "SHELL=/bin/sh LOGNAME=root HOME=/ USER=root PATH=/bin:/benchmark init=/usr/bin/busybox ostd.log_level=error icenet.addr=172.16.0.2/16 icenet.gw=172.16.0.1 -- sh";
            early_println!("[kcmdline] FDT bootargs missing/empty; using fallback: {}", fallback);
            fallback
        }
    };
    kernel_cmdline.call_once(|| args.into());
}

fn init_initramfs(initramfs: &'static Once<&'static [u8]>) {
    early_println!("[ostd::init_initramfs] start");
    if let Some((start, end)) = parse_initramfs_range() {
        let base_va = paddr_to_vaddr(start);
        let length = end - start;

        // Debug: print FDT-provided initrd range and linear-mapped VA.
        // NOTE: Avoid dereferencing early to reduce risk of faults before
        // the full kernel high mapping is established.
        early_println!(
            "[initramfs] FDT provided: paddr=[0x{:x}, 0x{:x}) len=0x{:x} va=0x{:x}",
            start,
            end,
            length,
            base_va,
        );

        initramfs.call_once(|| unsafe {
            core::slice::from_raw_parts(base_va as *const u8, length)
        });
        return;
    }

    unsafe {
        extern "Rust" {
            fn __asterinas_initramfs() -> &'static [u8];
            fn __asterinas_initramfs_range() -> (*const u8, usize);
        }
        let (ptr, len) = __asterinas_initramfs_range();
        // Avoid dereferencing the embedded payload pointer at this early stage.
        early_println!(
            "[initramfs] Falling back to embedded payload: ptr=0x{:x} len=0x{:x}",
            ptr as usize,
            len,
        );
        initramfs.call_once(|| __asterinas_initramfs());
    }
}

fn init_builtin_busybox(builtin_busybox: &'static Once<&'static [u8]>) {
    // Initialize the builtin busybox from embedded symbol if present.
    unsafe {
        extern "Rust" {
            fn __asterinas_busybox() -> &'static [u8];
            fn __asterinas_busybox_range() -> (*const u8, usize);
        }
        let (ptr, len) = __asterinas_busybox_range();
        if len != 0 {
            early_println!(
                "[busybox] builtin: ptr=0x{:x} len=0x{:x}",
                ptr as usize,
                len
            );
            builtin_busybox.call_once(|| __asterinas_busybox());
            return;
        }
    }
    builtin_busybox.call_once(|| &[]);
}

fn init_acpi_arg(acpi: &'static Once<BootloaderAcpiArg>) {
    early_println!("[ostd::init_acpi_arg] start");
    acpi.call_once(|| BootloaderAcpiArg::NotProvided);
}

fn init_framebuffer_info(_framebuffer_arg: &'static Once<BootloaderFramebufferArg>) {}

fn init_memory_regions(memory_regions: &'static Once<Vec<MemoryRegion>>) {
    early_println!("[ostd::init_memory_regions] start");
    let mut regions = Vec::<MemoryRegion>::new();

    // Prefer robust scanning: iterate memory nodes under root, ignore disabled ones.
    let fdt = DEVICE_TREE.get().unwrap();
    let mut found_any_memory = false;
    if let Some(root) = fdt.find_node("/") {
        early_println!("[ostd::init_memory_regions] scanning / children");
        for child in root.children() {
            let name = child.name;
            early_println!("  [mem-scan] node {}", name);
            let dev_type = child.property("device_type").and_then(|p| p.as_str());
            let is_mem = dev_type.is_some_and(|s| s == "memory");
            let is_disabled = child
                .property("status")
                .and_then(|p| p.as_str())
                .is_some_and(|s| s == "disabled");
            if is_mem && !is_disabled {
                if let Some(reg_iter) = child.reg() {
                    for r in reg_iter {
                        let size = r.size.unwrap_or(0);
                        if size > 0 {
                            regions.push(MemoryRegion::new(
                                r.starting_address as usize,
                                size,
                                MemoryRegionType::Usable,
                            ));
                            early_println!(
                                "    [mem-found] base=0x{:x} size=0x{:x}",
                                r.starting_address as u64,
                                size
                            );
                            found_any_memory = true;
                        }
                    }
                }
            }
        }
    }

    // Fallback to fdt.memory().regions() in case some platforms require it.
    if !found_any_memory {
        early_println!(
            "[ostd::init_memory_regions] fallback to fdt.memory().regions()"
        );
        for region in fdt.memory().regions() {
            if region.size.unwrap_or(0) > 0 {
                regions.push(MemoryRegion::new(
                    region.starting_address as usize,
                    region.size.unwrap(),
                    MemoryRegionType::Usable,
                ));
                early_println!(
                    "    [mem-found-fallback] base=0x{:x} size=0x{:x}",
                    region.starting_address as u64,
                    region.size.unwrap_or(0)
                );
                found_any_memory = true;
            }
        }
    }

    // Reserved memory ranges
    if let Some(node) = fdt.find_node("/reserved-memory") {
        early_println!("[ostd::init_memory_regions] scanning /reserved-memory");
        for child in node.children() {
            if let Some(reg_iter) = child.reg() {
                for region in reg_iter {
                    regions.push(MemoryRegion::new(
                        region.starting_address as usize,
                        region.size.unwrap(),
                        MemoryRegionType::Reserved,
                    ));
                    early_println!(
                        "    [reserved] base=0x{:x} size=0x{:x}",
                        region.starting_address as u64,
                        region.size.unwrap_or(0)
                    );
                }
            }
        }
    }

    // Add the kernel region.
    regions.push(MemoryRegion::kernel());

    // Add the initramfs region.
    let mut initramfs_tracked = false;
    if let Some((start, end)) = parse_initramfs_range() {
        regions.push(MemoryRegion::new(
            start,
            end - start,
            MemoryRegionType::Module,
        ));
        initramfs_tracked = true;
    }

    if !initramfs_tracked {
        unsafe {
            extern "Rust" {
                fn __asterinas_initramfs_range() -> (*const u8, usize);
            }
            let (ptr, len) = __asterinas_initramfs_range();
            if len != 0 {
                let phys = ptr as usize - mm::kspace::kernel_loaded_offset();
                regions.push(MemoryRegion::new(phys, len, MemoryRegionType::Module));
            }
        }
    }

    // Track builtin busybox region as module if present
    unsafe {
        extern "Rust" {
            fn __asterinas_busybox_range() -> (*const u8, usize);
        }
        let (ptr, len) = __asterinas_busybox_range();
        if len != 0 {
            let phys = ptr as usize - mm::kspace::kernel_loaded_offset();
            regions.push(MemoryRegion::new(phys, len, MemoryRegionType::Module));
        }
    }

    early_println!(
        "[ostd::init_memory_regions] total collected regions: {}",
        regions.len()
    );
    memory_regions.call_once(|| non_overlapping_regions_from(regions.as_ref()));
    early_println!("[ostd::init_memory_regions] call_once completed");
}

fn parse_initramfs_range() -> Option<(usize, usize)> {
    let fdt = DEVICE_TREE.get().unwrap();
    let chosen = fdt.find_node("/chosen")?;
    let initrd_start = chosen.property("linux,initrd-start")?.as_usize()?;
    let initrd_end = chosen.property("linux,initrd-end")?.as_usize()?;
    Some((initrd_start, initrd_end))
}

/// The entry point of the Rust code portion of Asterinas.
#[no_mangle]
pub extern "C" fn riscv_boot(_hart_id: usize, device_tree_paddr: usize) -> ! {
    early_println!("[ostd::riscv_boot] Enter riscv_boot");
    // Record the boot hart's hardware ID as early as possible.
    BOOT_HART_ID.call_once(|| _hart_id);

    let device_tree_ptr = paddr_to_vaddr(device_tree_paddr) as *const u8;
    early_println!("[ostd::riscv_boot] out of paddr_to_vaddr {} : {}, hardid : {}", device_tree_paddr, device_tree_ptr as usize, _hart_id);
    let fdt = unsafe { fdt::Fdt::from_ptr(device_tree_ptr).unwrap() };
    early_println!("[ostd::riscv_boot] out of from ptr");
    DEVICE_TREE.call_once(|| fdt);
    early_println!("[ostd::riscv_boot] out of call once");

    crate::boot::register_boot_init_callbacks(
        init_bootloader_name,
        init_kernel_commandline,
        init_initramfs,
        init_builtin_busybox,
        init_acpi_arg,
        init_framebuffer_info,
        init_memory_regions,
    );
    early_println!("[ostd::riscv_boot] out of register_boot_init_callbacks");

    crate::boot::call_ostd_main();
}

#[no_mangle]
extern "C" fn boot_trap_handler(scause: usize, stval: usize) -> ! {
    // Print basic trap info first.
    early_println!("[boot-trap] scause=0x{:x} stval=0x{:x}", scause, stval);

    // Decode scause using CSR to get typed cause info.
    let sc = riscv::register::scause::read();
    let cause = sc.cause();

    // Snapshot key CSRs to help diagnose early faults.
    let epc = riscv::register::sepc::read();
    let satp_val = riscv::register::satp::read();
    let stvec_val = riscv::register::stvec::read();
    let sscratch_val = riscv::register::sscratch::read();

    early_println!(
        "[boot-trap] cause={:?} sepc=0x{:x}",
        cause,
        epc
    );
    early_println!(
        "[boot-trap] satp: mode={:?} asid=0x{:x} ppn=0x{:x}",
        satp_val.mode(),
        satp_val.asid(),
        satp_val.ppn()
    );
    // stvec encoding: [1:0]=mode (0=Direct,1=Vectored), [XLEN-1:2]=base
    let stvec_bits = stvec_val.bits();
    let stvec_base = stvec_bits & !0x3usize;
    let stvec_mode_raw = stvec_bits & 0x3;
    let stvec_mode_str = match stvec_mode_raw {
        0 => "Direct",
        1 => "Vectored",
        _ => "Reserved",
    };
    early_println!(
        "[boot-trap] stvec: base=0x{:x} mode={} sscratch=0x{:x}",
        stvec_base,
        stvec_mode_str,
        sscratch_val
    );
    // Extra: walk sepc/stval through current SATP root to diagnose mapping.
    debug_walk_va("sepc", epc as usize);
    debug_walk_va("stval", stval as usize);
    loop {
        unsafe { riscv::asm::wfi() };
    }
}

/// Debug: read-only walk of Sv48 page tables from current SATP and dump PTEs.
fn debug_walk_va(tag: &str, vaddr: usize) {
    use riscv::register::satp::Mode;
    let satp = riscv::register::satp::read();
    early_println!(
        "[ptwalk] {} va=0x{:x} satp: mode={:?} ppn=0x{:x}",
        tag,
        vaddr,
        satp.mode(),
        satp.ppn()
    );
    if satp.mode() != Mode::Sv48 {
        early_println!("[ptwalk] non-Sv48 mode; skip walk");
        return;
    }

    // Indices for Sv48: [47:39],[38:30],[29:21],[20:12]
    let idx = [
        (vaddr >> 39) & 0x1ff,
        (vaddr >> 30) & 0x1ff,
        (vaddr >> 21) & 0x1ff,
        (vaddr >> 12) & 0x1ff,
    ];
    let mut table_pa = (satp.ppn() as usize) << 12;
    let mut level = 4usize;

    for &i in &idx {
        let table_va = crate::mm::paddr_to_vaddr(table_pa);
        let pte_va = table_va + (i as usize) * core::mem::size_of::<usize>();
        let pte_val = unsafe { core::ptr::read_volatile(pte_va as *const usize) };
        early_println!(
            "[ptwalk] L{} idx={} table_pa=0x{:x} pte=0x{:x}",
            level,
            i,
            table_pa,
            pte_val
        );

        let valid = (pte_val & 0x1) != 0;
        let rwx = (pte_val >> 1) & 0x7; // R/W/X bits
        // PPN in Sv48 PTE spans bits [53:10]. Mask per OSTD's definition.
        let ppn = (pte_val & 0x003F_FFFF_FFFF_FC00) >> 10;
        let next_pa = (ppn as usize) << 12;

        if !valid {
            early_println!("[ptwalk] -> INVALID at L{}", level);
            return;
        }
        if rwx != 0 {
            // Leaf PTE; compute effective PA for info only.
            let off_bits = 12 + (level - 1) * 9;
            let page_off_mask = if off_bits >= (core::mem::size_of::<usize>() * 8) {
                usize::MAX
            } else {
                (1usize << off_bits) - 1
            };
            let eff_pa = next_pa | (vaddr & page_off_mask);
            early_println!(
                "[ptwalk] -> LEAF at L{} next_pa=0x{:x} eff_pa=0x{:x}",
                level,
                next_pa,
                eff_pa
            );
            return;
        }
        table_pa = next_pa;
        if level == 1 {
            break;
        }
        level -= 1;
    }
    early_println!("[ptwalk] walk finished");
}
