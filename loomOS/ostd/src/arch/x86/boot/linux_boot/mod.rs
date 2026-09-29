// SPDX-License-Identifier: MPL-2.0

//! The RISC-V boot module defines the entrypoints of Asterinas.

#[cfg(target_arch = "riscv64")]
use rustsbi;

pub mod smp;

use alloc::{string::String, vec::Vec};
use core::{arch::global_asm, arch::asm};

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
/// The linear-mapped base virtual address of the DTB blob.
pub static DEVICE_TREE_BASE_VA: Once<usize> = Once::new();

/// The hardware hart ID of the boot hart (passed by firmware via a0).
///
/// Stored at the very beginning of `riscv_boot` so other modules (e.g., SMP
/// bring-up) can avoid re-starting the BSP via SBI HSM.
pub static BOOT_HART_ID: Once<usize> = Once::new();

#[inline(always)]
fn read_gp() -> usize {
    let mut v: usize;
    unsafe { asm!("mv {}, gp", out(reg) v, options(nomem, nostack, preserves_flags)) }
    v
}

#[inline(always)]
fn read_tp() -> usize {
    let mut v: usize;
    unsafe { asm!("mv {}, tp", out(reg) v, options(nomem, nostack, preserves_flags)) }
    v
}

fn init_bootloader_name(bootloader_name: &'static Once<String>) {
    early_println!("[ostd::init_bootloader_name] start");

    // Extra diagnostics to locate early faults
    let gp = read_gp();
    let tp = read_tp();
    let arg_ptr = bootloader_name as *const Once<String> as usize;
    let satp_val = riscv::register::satp::read();
    let stvec_val = riscv::register::stvec::read();
    let stvec_bits = stvec_val.bits();
    let stvec_base = stvec_bits & !0x3usize;
    let stvec_mode_raw = stvec_bits & 0x3;
    let stvec_mode_str = match stvec_mode_raw { 0 => "Direct", 1 => "Vectored", _ => "Reserved" };
    let u: &'static str = "Unknown";
    let u_ptr = u.as_ptr() as usize;
    let u_len = u.len();

    early_println!(
        "[boot.name] gp=0x{:x} tp=0x{:x} once_arg=0x{:x} u_ptr=0x{:x} u_len={} satp.mode={:?} ppn=0x{:x}",
        gp,
        tp,
        arg_ptr,
        u_ptr,
        u_len,
        satp_val.mode(),
        satp_val.ppn()
    );
    early_println!(
        "[boot.name] stvec.base=0x{:x} stvec.mode={}",
        stvec_base,
        stvec_mode_str
    );

    bootloader_name.call_once(|| {
        // Print inside closure just before constructing the String
        let gp_in = read_gp();
        let tp_in = read_tp();
        early_println!("[boot.name] in-closure gp=0x{:x} tp=0x{:x}", gp_in, tp_in);
        // Avoid heap allocation at this very early stage to reduce
        // the chance of touching unmapped addresses via the allocator.
        // Use an empty String as a placeholder; it can be updated later.
        String::new()
    });

    early_println!("[ostd::init_bootloader_name] bootloader name finish");
}

fn init_kernel_commandline(kernel_cmdline: &'static Once<KCmdlineArg>) {
    early_println!("[ostd::init_kernel_commandline] start");
    // Print DTB VA and header magic for sanity before walking the tree.
    if let Some(&dtb_va) = DEVICE_TREE_BASE_VA.get() {
        unsafe {
            let hdr = core::ptr::read_volatile(dtb_va as *const u32);
            early_println!(
                "[kcmd] DTB @va=0x{:x} hdr magic=0x{:08x}",
                dtb_va,
                u32::from_be(hdr)
            );
        }
    } else {
        early_println!("[kcmd] DTB base VA unknown");
    }

    let fdt_ref = DEVICE_TREE.get().unwrap();
    early_println!("[kcmd] DEVICE_TREE.get() ok");
    let chosen = fdt_ref.chosen();
    early_println!("[kcmd] fdt.chosen() ok");
    let bootargs = chosen.bootargs().unwrap_or("");
    early_println!("[kcmd] bootargs slice acquired (len={})", bootargs.len());
    kernel_cmdline.call_once(|| bootargs.into());
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
    DEVICE_TREE_BASE_VA.call_once(|| device_tree_ptr as usize);
    DEVICE_TREE.call_once(|| fdt);
    early_println!("[ostd::riscv_boot] out of call once");

    crate::boot::register_boot_init_callbacks(
        init_bootloader_name,
        init_kernel_commandline,
        init_initramfs,
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
    loop {
        unsafe { riscv::asm::wfi() };
    }
}
