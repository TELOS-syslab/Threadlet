// SPDX-License-Identifier: MPL-2.0

//! Multiprocessor Boot Support

// Take a typical OpenSBI and Linux based system as an example to break down the entire boot process.
// - Phase 1: Firmware Initialization (M-mode) 
// Power-up/Reset: All harts execute the firmware code (e.g. _start for OpenSBI) 
// starting from the reset vector at the same time.
// Boot Hart Selection: 
// The firmware code reads the mhartid CSR (Control and Status Register), which is a unique hardware ID for each hart. 
// elects a hart as the primary boot core by convention (usually the smallest mhartid, e.g. 0).
// Parking Secondary Harts: 
// All non-primary harts perform a simple “Parking Loop”.
// In this loop, they call the WFI (Wait For Interrupt) instruction, which puts them into a low-power sleep state, waiting to be woken up by an interrupt.
// Key point: Before entering WFI, the firmware prepares a “context” for each secondary core. A pointer to the core's per-hart data is usually stored in the mscratch CSR. This data area contains the address to jump to in case of a future wakeup (to be provided by the operating system).
// The main boot core performs the initialization: 
// The main boot core then proceeds to perform the complete firmware initialization, e.g. initializing the serial port, setting up the PMP (Physical Memory Protection), probing the memory, etc. The main boot core then proceeds to perform the complete firmware initialization.
// Once the initialization is complete, the firmware loads the next stage of the bootloader (usually U-Boot or just the Linux Kernel) into memory.
// Finally, the firmware jumps from M-mode to S-mode, handing over control to the OS kernel.
// - Phase 2: OS Boot and Wake-up of Secondary Kernel (S-mode) 
// The primary kernel runs the operating system: 
// The Linux kernel starts executing its start_kernel function on the primary boot kernel (e.g. Hart 0).
// The kernel performs various initializations of itself, such as memory management, scheduler, device drivers, and so on.
// Getting ready to wake up the secondary kernel: 
// When the kernel has reached a certain stage of initialization and is ready to support multiple cores (in Linux this is usually triggered by functions such as smp_init() -> cpu_up()), it starts booting the secondary kernel.
// Initiating a wakeup request (SBI Call): 
// When the Linux kernel needs to wake up a sub-core (e.g. Hart 1), it can't manipulate the hardware directly because it is running in S-mode and doesn't have access to M-mode resources.
// It constructs an SBI call, which is sbi_send_ipi() (Send Inter-Processor Interrupt).
// The kernel encodes the hart ID to be woken up into a hart mask and then executes the ecall instruction. This instruction creates a trap, temporarily handing control from S-mode to OpenSBI in M-mode. 
// - Phase 3: Firmware Response and Wakeup (M-mode) 
// SBI Call Handling: 
// OpenSBI's ecall trap handler is triggered. It resolves this as a send_ipi request.
// Send Inter-core Interrupt (IPI): 
// OpenSBI sends a “Machine Software Interrupt” to the target hart.
// This is done by operating a standard hardware module CLINT (Core-Local Interrupter). Each hart has a 32-bit msip (Machine Software Interrupt Pending) register in CLINT.
// OpenSBI triggers the interrupt by writing a 1 to the address of the msip register corresponding to the target hart.

// It jumps to the M-mode interrupt handler (also within OpenSBI).
// - Phase 4. Jump to the operating system: 
// OpenSBI's interrupt handler finds out that this is an IPI for wakeup. 
// It will take the previously saved context information from the mscratch register, which contains the Linux kernel's entry address for this subkernel.
// OpenSBI sets up the S-mode execution context (e.g., puts the kernel entry address into sepc, puts the hart id into a1), and then executes the sret instruction, which hands over control from the M-mode to the S-mode's Linux kernel.


use core::{arch::global_asm, panic};

use crate::{arch::boot::DEVICE_TREE, early_println};
use alloc::vec::Vec;
use spin::Once;
use core::sync::atomic::{AtomicBool, Ordering};

// Include the assembly code for the AP entry point.
// This makes the `_ap_startup` symbol available to the Rust code.
global_asm!(include_str!("ap_entry.S"));

/// Hardware hart IDs discovered from device tree (index == logical CPU index).
pub(crate) static HART_ID_LIST: Once<Vec<u32>> = Once::new();
/// Threadlet count per CPU parsed from FDT (index == logical CPU index).
static THREADLET_COUNT_PER_CPU: Once<Vec<u32>> = Once::new();
/// Whether any CPU node reports non-zero `threadlet` property in FDT.
static THREADLET_PRESENT: Once<bool> = Once::new();
/// Interrupt threadlet hart id per CPU parsed from FDT (index == logical CPU index).
static INTR_THREADLET_PER_CPU: Once<Vec<Option<u32>>> = Once::new();
/// Whether any CPU node provides an interrupt-dedicated threadlet hart.
static INTR_THREADLET_PRESENT: Once<bool> = Once::new();

/// Query whether any CPU reports hardware threadlets in the device tree.
pub fn threadlet_present() -> bool {
    if let Some(p) = THREADLET_PRESENT.get() {
        *p
    } else {
        false
    }
}

/// Returns the recorded threadlet count per CPU if available.
pub fn threadlet_counts() -> Option<&'static Vec<u32>> {
    THREADLET_COUNT_PER_CPU.get()
}

/// Query whether any CPU reports an interrupt-dedicated threadlet hart.
pub fn intr_threadlet_present() -> bool {
    INTR_THREADLET_PRESENT.get().copied().unwrap_or(false)
}

/// Returns the interrupt threadlet hart id for the given logical CPU index if available.
pub fn intr_threadlet_for_cpu_index(cpu_index: usize) -> Option<u32> {
    INTR_THREADLET_PER_CPU
        .get()
        .and_then(|vec| vec.get(cpu_index))
        .and_then(|v| *v)
}

/// Returns the interrupt threadlet hart id for the given hardware hart id if available.
pub fn intr_threadlet_for_hart(hartid: u32) -> Option<u32> {
    let Some(list) = HART_ID_LIST.get() else { return None };
    let Some(intr_vec) = INTR_THREADLET_PER_CPU.get() else { return None };
    list.iter()
        .zip(intr_vec.iter())
        .find_map(|(&hid, intr)| if hid == hartid { *intr } else { None })
}

/// Get the number of processors from the device tree
pub(crate) fn get_num_processors() -> Option<u32> {
    let fdt = DEVICE_TREE.get()?;
    
    early_println!("\n===== CPU Information from Device Tree =====");
    
    let cpus_node = fdt.find_node("/cpus")?; 
    // #address-cells and #size-cells
    let address_cells = cpus_node.property("#address-cells")
        .and_then(|p| p.as_usize())
        .unwrap_or(1);
    let size_cells = cpus_node.property("#size-cells")
        .and_then(|p| p.as_usize())
        .unwrap_or(0);
    
    early_println!("CPUs node: #address-cells={}, #size-cells={}", address_cells, size_cells);
    
    // get all cpu nodes
    let mut cpu_count = 0;
    let mut active_cpus = 0;
    let mut hart_ids: Vec<u32> = Vec::new();
    let mut threadlet_counts: Vec<u32> = Vec::new();
    let mut intr_threadlet_ids: Vec<Option<u32>> = Vec::new();
    
    for cpu in fdt.find_all_nodes("/cpus/cpu") {
        cpu_count += 1;
        
        // hartid
        let hartid = cpu.property("reg").and_then(|p| p.as_usize()).unwrap_or(cpu_count - 1);
        let status = cpu.property("status").and_then(|p| p.as_str()).unwrap_or("okay");
        
        let isa = cpu.property("riscv,isa").and_then(|p| p.as_str()).unwrap_or("unknown");
        
        // MMU TYPE
        let mmu = cpu.property("mmu-type").and_then(|p| p.as_str()).unwrap_or("unknown");
        
        let timebase_frequency = cpu.property("timebase-frequency")
            .and_then(|p| p.as_usize())
            .unwrap_or(0);
        
        let intr_threadlet = cpu
            .property("intr-threadlet")
            .and_then(|p| p.as_usize())
            .unwrap_or(0);
        // Non-standard: detect hardware threadlet count if provided by FDT.
        let threadlet = cpu
            .property("threadlet")
            .and_then(|p| p.as_usize())
            .unwrap_or(0);

        early_println!(
            "CPU {}: hartid={}, status={} threadlet_count={} intr_threadlet={}",
            cpu_count - 1,
            hartid,
            status,
            threadlet,
            intr_threadlet
        );
        early_println!("isa={}, mmu-type={}, timebase-frequency={} Hz", isa, mmu,timebase_frequency);
        
        if status == "okay" {
            active_cpus += 1;
        }

        // Record hartid (even if disabled) to keep index mapping stable.
        hart_ids.push(hartid as u32);
        threadlet_counts.push(threadlet as u32);
        intr_threadlet_ids.push(if intr_threadlet > 0 {
            Some(intr_threadlet as u32)
        } else {
            None
        });
    }
    early_println!("Active cpus:{}", active_cpus);
    // Store hart-id map exactly once for later SMP bring-up.
    if HART_ID_LIST.get().is_none() {
        HART_ID_LIST.call_once(|| hart_ids);
    }
    if THREADLET_COUNT_PER_CPU.get().is_none() {
        let has_threadlet = threadlet_counts.iter().any(|&c| c > 0)
            || intr_threadlet_ids.iter().any(|v| v.is_some());
        THREADLET_COUNT_PER_CPU.call_once(|| threadlet_counts);
        THREADLET_PRESENT.call_once(|| has_threadlet);
    }
    if INTR_THREADLET_PER_CPU.get().is_none() {
        let has_intr_threadlet = intr_threadlet_ids.iter().any(|v| v.is_some());
        INTR_THREADLET_PER_CPU.call_once(|| intr_threadlet_ids);
        INTR_THREADLET_PRESENT.call_once(|| has_intr_threadlet);
    }
    
    
    if cpu_count > 0 {
        Some(cpu_count as u32)
    } else {
        early_println!("No CPUs found in device tree, defaulting to 1");
        Some(1)
    }
}


pub const TRAMPOLINE_PA: usize = 0x90000000;
const KERNEL_VMA_OFFSET: usize = 0xFFFFFFFF00000000;
const LINEAR_MAPPING_BASE_VADDR: usize = 0xFFFF800000000000;

// for debug
fn copy_trampoline_code() -> usize {
    unsafe extern "C" {
        static __trampoline_start: u8;
        static __trampoline_end: u8;
        static __ap_startup_offset: u8; 
        static _ap_startup: u8; 
    }
    
    let trampoline_start = unsafe { &__trampoline_start as *const u8 as usize };
    let trampoline_end = unsafe { &__trampoline_end as *const u8 as usize };
    let ap_startup_offset = unsafe { &__ap_startup_offset as *const u8 as usize };
    let ap_startup_addr = unsafe { &_ap_startup as *const u8 as usize };

    crate::early_print!("[copy_trampoline_code] 0x{:x} to 0x{:x} \n", trampoline_start, trampoline_end);
    let len = trampoline_end - trampoline_start;
    
    crate::early_print!("[copy_trampoline_code] copying {} bytes from 0x{:x} to 0x{:x}\n", 
                       len, trampoline_start, crate::mm::paddr_to_vaddr(TRAMPOLINE_PA));
    crate::early_print!("[copy_trampoline_code] _ap_startup offset: 0x{:x}\n", ap_startup_offset);
    crate::early_print!("[copy_trampoline_code] _ap_startup addr: 0x{:x}\n", ap_startup_addr);


    unsafe {
        let src_ptr = trampoline_start as *const u32;
        crate::early_print!("[copy_trampoline_code] source first 4 instructions: 0x{:08x} 0x{:08x} 0x{:08x} 0x{:08x}\n",
                           src_ptr.read_volatile(),
                           src_ptr.add(1).read_volatile(),
                           src_ptr.add(2).read_volatile(),
                           src_ptr.add(3).read_volatile());
    }

    let dest_vaddr = crate::mm::paddr_to_vaddr(TRAMPOLINE_PA);
    crate::early_print!("[copy_trampoline_code] TRAMPOLINE_PA: 0x{:x}, dest_vaddr: 0x{:x}\n", 
                       TRAMPOLINE_PA, dest_vaddr);


    unsafe {
        let test_ptr = dest_vaddr as *mut u32;
        
        test_ptr.write_volatile(0xDEADBEEF);
        let read_back = test_ptr.read_volatile();
        crate::early_print!("[copy_trampoline_code] write test: wrote 0xDEADBEEF, read back 0x{:08x}\n", read_back);
    }

    

    unsafe {
        core::ptr::copy_nonoverlapping(
            trampoline_start as *const u8,
            crate::mm::paddr_to_vaddr(TRAMPOLINE_PA) as *mut u8,
            len,
        );
        core::arch::asm!("fence.i");
    }


    unsafe {
        let dst_ptr = crate::mm::paddr_to_vaddr(TRAMPOLINE_PA) as *const u32;
        crate::early_print!("[copy_trampoline_code] dest first 4 instructions: 0x{:08x} 0x{:08x} 0x{:08x} 0x{:08x}\n",
                           dst_ptr.read_volatile(),
                           dst_ptr.add(1).read_volatile(),
                           dst_ptr.add(2).read_volatile(),
                           dst_ptr.add(3).read_volatile());
    }
    // let ap_entry_pa = TRAMPOLINE_PA + (ap_startup_addr - trampoline_start);
    // crate::early_print!("[copy_trampoline_code] AP entry point: 0x{:x}\n", ap_entry_pa);
    
    // ap_entry_pa
    ap_startup_addr - KERNEL_VMA_OFFSET
}


const AP_STACK_SIZE: usize = 0x40000;
unsafe extern "C" {
    fn ap_early_entry(hartid: usize) -> !;
}
pub(crate) fn bringup_all_aps() {
    crate::early_print!("[ostd::bringup_all_aps] start!\n");

    // Get the boot info prepared by the platform-agnostic `boot_all_aps` function.
    let boot_info = crate::boot::smp::AP_BOOT_INFO.get()
        .expect("AP_BOOT_INFO not initialized");
    let boot_stack_array_paddr = boot_info.boot_stack_array.start_paddr();
    let boot_stack_array_vaddr = crate::mm::paddr_to_vaddr(boot_stack_array_paddr);
    crate::early_print!("[ostd::bringup_all_aps] boot_stack_array_vaddr: {:#x}\n", boot_stack_array_vaddr);

    extern "C" { fn _ap_startup(); }
    let ap_entry_pa = _ap_startup as usize - KERNEL_VMA_OFFSET;
    crate::early_print!("[ostd::bringup_all_aps] ap_entry_pa: {:#x}\n", ap_entry_pa);

    let ap_early_entry_vaddr = crate::boot::smp::ap_early_entry as usize;
    crate::early_print!("[ostd::bringup_all_aps] ap_early_entry_vaddr: {:#x}\n", ap_early_entry_vaddr);

    let kernel_satp_value = riscv::register::satp::read().bits();
    crate::early_print!("[ostd::bringup_all_aps] current satp value: {:#x}\n", kernel_satp_value);
    
    // Start all APs.
    let total_cpus = crate::cpu::num_cpus();
    let hart_ids = HART_ID_LIST
        .get()
        .expect("HART_ID_LIST must be initialized by get_num_processors()");
    let bsp_hartid = crate::arch::boot::BOOT_HART_ID.get().copied().unwrap_or(0);
    crate::early_print!(
        "[ostd::bringup_all_aps] bsp_hartid={} total_cpus={}\n",
        bsp_hartid,
        total_cpus
    );

    // Build AP list in OS logical order: OS CPU ID 1..N-1 map to
    // hardware hart IDs excluding the BSP hart.
    let mut ap_os_id: usize = 1;
    for &hartid_u32 in hart_ids.iter().take(total_cpus) {
        let hartid = hartid_u32 as usize;
        if hartid == bsp_hartid { continue; }
        let os_cpu_id = ap_os_id;
        ap_os_id += 1;

        // Get the pre-allocated stack top for this hart from the boot_stack_array.
        // Read the pre-allocated boot stack pointer for this AP using
        // OS logical CPU ID as index (matches writer in ostd/src/boot/smp.rs).
        let ap_private_stack_top_vaddr = unsafe {
            (boot_stack_array_vaddr as *const u64)
                .add(os_cpu_id)
                .read_volatile()
        } as usize;
        let ap_private_stack_top_paddr = ap_private_stack_top_vaddr - LINEAR_MAPPING_BASE_VADDR;
        crate::early_print!(
            "[ostd::bringup_all_aps] hart {} (os_cpu_id={}) ap_private_stack_top_vaddr={:#x} paddr={:#x}\n",
            hartid, os_cpu_id, ap_private_stack_top_vaddr, ap_private_stack_top_paddr
        );

        // Write necessary information to the top of the AP's stack for _ap_startup to use.
        unsafe {
            // Write the virtual address of the Rust entry function.
            let entry_addr_ptr = (ap_private_stack_top_vaddr as *mut u64).offset(-1);
            entry_addr_ptr.write_volatile(ap_early_entry_vaddr as u64);
            
            // Write the kernel's page table pointer (satp).
            let satp_addr_ptr = (ap_private_stack_top_vaddr as *mut u64).offset(-2);
            satp_addr_ptr.write_volatile(kernel_satp_value as u64);

            // Write the virtual address of the stack top itself.
            let stack_addr_ptr = (ap_private_stack_top_vaddr as *mut u64).offset(-3);
            stack_addr_ptr.write_volatile(ap_private_stack_top_vaddr as u64);
        }
        
        crate::early_print!(
            "[ostd::bringup_all_aps] starting hart {} (os_cpu_id={}) with entry={:#x}, stack_paddr={:#x}\n",
            hartid, os_cpu_id, ap_entry_pa, ap_private_stack_top_paddr
        );

        let ret = sbi_rt::hart_start(
            hartid,
            ap_entry_pa,
            ap_private_stack_top_paddr,
        );

        if ret.error == sbi_spec::binary::RET_SUCCESS {
            crate::early_print!("[ostd::bringup_all_aps] hart {} start command sent successfully\n", hartid);
        } else {
            // Decode common SBI error codes for readability.
            let err = ret.error as isize;
            let hint = match err {
                -6 => "already started (OpenSBI HSM)",
                -7 => "already started",
                -5 => "invalid start address",
                -4 => "denied",
                -3 => "invalid parameter (hartid?)",
                -2 => "not supported",
                -1 => "failed",
                _ => "unknown",
            };
            crate::early_print!(
                "[ostd::bringup_all_aps] hart {} start failed, error={} ({})\n",
                hartid, err, hint
            );
        }
    }
    
    crate::early_print!("[ostd::bringup_all_aps] finish!\n");
}




pub(crate) fn wait_for_all_aps_started_for_test() {
    let total_cpus = crate::cpu::num_cpus();
    let bsp_cpu_id = crate::cpu::CpuId::bsp().id();
    let expected_aps = total_cpus - 1; 
    
    crate::early_print!("[wait_for_all_aps_started] waiting for {} APs to start\n", expected_aps);
    unsafe extern "C" {
        static __ap_stacks_start: u8;
    }
    let ap_stacks_start_vaddr = unsafe { &__ap_stacks_start as *const u8 as usize };
    let max_wait_cycles = 20000000; 
    
    for _ in 0..max_wait_cycles {
        core::hint::spin_loop();
    }
    
    let mut started_count = 0;
    crate::early_print!("[wait_for_all_aps_started] checking all APs after waiting\n");

    // let ap_early_entry_vaddr = ap_early_entry as usize;
    
    for hartid in 0..total_cpus {
        if hartid == bsp_cpu_id as usize {
            continue;
        }
        let ap_private_stack_top_vaddr = ap_stacks_start_vaddr + hartid * AP_STACK_SIZE;
        early_println!("hartid: {}, ap_private_stack_top_vaddr: 0x{:x}\n", hartid, ap_private_stack_top_vaddr);
        let flag_addr = ap_private_stack_top_vaddr - 2000;
        
        unsafe {
            let flag_value = (flag_addr as *const u64).read_volatile();
            let expected_flag = 0xAC00u64 + hartid as u64;
            
            if flag_value == expected_flag {
                crate::early_print!("[wait_for_all_aps_started] hart {} started (flag: 0x{:x})\n", 
                                   hartid, flag_value);
                started_count += 1;
            } else {
                crate::early_print!("[wait_for_all_aps_started] hart {} not started (flag: 0x{:x}, expected: 0x{:x})\n", 
                                   hartid, flag_value, expected_flag);
                panic!("AP start failed!");
            }
        }
    }
    
    if started_count == expected_aps {
        crate::early_print!("[wait_for_all_aps_started] all {} APs started successfully!\n", expected_aps);
    } else {
        crate::early_print!("[wait_for_all_aps_started] only {}/{} APs started\n", 
                           started_count, expected_aps);
    }
}



//    .trampoline : AT(KERNEL_LMA - 0x10000) {
//         PROVIDE(__trampoline_start = KERNEL_VMA - 0x10000);

//         KEEP(*(.trampoline .trampoline.*))

//         . = ALIGN(4096);
//         PROVIDE(__trampoline_stack_bottom = .);
//         . += 4096;
//         PROVIDE(__trampoline_stack_top = .);
//     }


// pub(crate) fn bringup_all_aps() {
//     crate::early_print!("[ostd::bringup_all_aps] start!\n");

//     unsafe extern "C" {
//         static __ap_stacks_start: u8;
//     }
//     let ap_stacks_start_vaddr = unsafe { &__ap_stacks_start as *const u8 as usize };
//     crate::early_print!("[ostd::bringup_all_aps] ap_stacks_start_vaddr: 0x{:x}\n", ap_stacks_start_vaddr);
//     let ap_stacks_start_paddr = ap_stacks_start_vaddr -  KERNEL_VMA_OFFSET as usize;

//     let ap_entry_pa = copy_trampoline_code();
//     crate::early_print!("[ostd::bringup_all_aps] ap_entry_pa: 0x{:x}\n", ap_entry_pa);

//     let ap_early_entry_vaddr = ap_early_entry as usize;
//     crate::early_print!("[ostd::bringup_all_aps] ap_early_entry_vaddr: 0x{:x}\n", ap_early_entry_vaddr);


//     let kernel_satp_value = riscv::register::satp::read().bits();
//     crate::early_print!("[ostd::bringup_all_aps] current satp value: 0x{:x}\n", kernel_satp_value);
    

//     let bsp_cpu_id = crate::cpu::CpuId::bsp().id();
//     let total_cpus = crate::cpu::num_cpus();
    
//     for hartid in 0..total_cpus {
//         if hartid == bsp_cpu_id as usize {
//             continue;
//         }
//         let ap_private_stack_top_paddr = ap_stacks_start_paddr + hartid * AP_STACK_SIZE;
//         let ap_private_stack_top_vaddr = ap_stacks_start_vaddr + hartid * AP_STACK_SIZE;
//         // write related addr to AP stack
//         unsafe{
//             let entry_addr_ptr = (ap_private_stack_top_vaddr - 8) as *mut u64;
//             entry_addr_ptr.write_volatile(ap_early_entry_vaddr as u64);
            

//             let satp_addr_ptr = (ap_private_stack_top_vaddr - 16) as *mut u64;
//             satp_addr_ptr.write_volatile(kernel_satp_value as u64);

//             let stack_addr_ptr = (ap_private_stack_top_vaddr - 24) as *mut u64;
//             stack_addr_ptr.write_volatile(ap_private_stack_top_vaddr  as u64);
//         }
        


//         crate::early_print!("[ostd::bringup_all_aps] starting hart {} with entry=0x{:x}, trampoline=0x{:x}\n", 
//                            hartid, ap_entry_pa, ap_private_stack_top_paddr);

//         let ret = sbi_rt::hart_start(
//             hartid,
//             ap_entry_pa,    // ap entry phys addr
//             ap_private_stack_top_paddr,  // opaque
//         );

//         if ret.error == sbi_spec::binary::RET_SUCCESS {
//             crate::early_print!("[ostd::bringup_all_aps] hart {} start command sent successfully\n", hartid);
//         } else {
//             crate::early_print!("[ostd::bringup_all_aps] hart {} start failed, error={}\n", hartid, ret.error);
//         }
//     }
    
//     crate::early_print!("[ostd::bringup_all_aps] finish!\n");
// }
