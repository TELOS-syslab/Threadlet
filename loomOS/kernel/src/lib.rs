// SPDX-License-Identifier: MPL-2.0

//! Aster-nix is the Asterinas kernel, a safe, efficient unix-like
//! operating system kernel built on top of OSTD and OSDK.

#![no_std]
#![no_main]
// #![deny(unsafe_code)]
#![allow(incomplete_features)]
#![feature(btree_cursors)]
#![feature(btree_extract_if)]
#![feature(debug_closure_helpers)]
#![feature(extend_one)]
#![feature(fn_traits)]
#![feature(format_args_nl)]
#![feature(int_roundings)]
#![feature(let_chains)]
#![feature(linked_list_cursors)]
#![feature(linked_list_remove)]
#![feature(linked_list_retain)]
#![feature(negative_impls)]
#![feature(panic_can_unwind)]
#![feature(register_tool)]
// FIXME: This feature is used to support vm capbility now as a work around.
// Since this is an incomplete feature, use this feature is unsafe.
// We should find a proper method to replace this feature with min_specialization, which is a sound feature.
#![feature(specialization)]
#![feature(step_trait)]
#![feature(trait_alias)]
#![feature(trait_upcasting)]
#![register_tool(component_access_control)]

use ostd::{
    arch::qemu::{exit_qemu, QemuExitCode},
    boot,
    cpu::{CpuId, CpuSet, PinCurrentCpu},
};
use process::Process;

use crate::{
    prelude::*,
    sched::priority::Priority,
    thread::{Thread, kernel_thread::ThreadOptions}, threadlet_arch::do_nop_cycle_test,
};
use aster_bigtcp::wire::Ipv4Address;
use ostd::mm::{UntypedMem, VmIoOnce, VmReader, VmWriter};
use crate::net::socket::{MessageHeader, Socket, SocketAddr, SendRecvFlags};
use ostd::boot::{kernel_cmdline, kcmdline::ModuleArg};
use aster_softirq::{softirq_id::ICENET_SOFTIRQ_PRIO};
#[cfg(target_arch = "riscv64")]
use core::sync::atomic::{AtomicU64, Ordering};
#[cfg(target_arch = "riscv64")]
use core::hint::black_box;
#[cfg(target_arch = "riscv64")]
use ostd::mm::{FrameAllocOptions, Segment, VmIo, PAGE_SIZE};
#[cfg(target_arch = "riscv64")]
use spin::Once as SpinOnce;
#[cfg(target_arch = "riscv64")]
use crate::threadlet_syscall::*;

extern crate alloc;
extern crate lru;
#[macro_use]
extern crate controlled;
#[macro_use]
extern crate getset;

#[cfg(target_arch = "riscv64")]
mod embedded_initramfs {
    #![allow(unsafe_code)]

    /// Embedded initramfs payload used when firmware does not provide one.
    pub static INITRAMFS: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../test/build/initramfs.cpio.gz"
    ));

    #[allow(unsafe_code)]
    #[export_name = "__asterinas_initramfs"]
    pub extern "Rust" fn initramfs_blob() -> &'static [u8] {
        INITRAMFS
    }

    #[allow(unsafe_code)]
    #[export_name = "__asterinas_initramfs_range"]
    pub extern "Rust" fn initramfs_range() -> (*const u8, usize) {
        (INITRAMFS.as_ptr(), INITRAMFS.len())
    }

    /// Embedded busybox payload to bypass slow initramfs extraction on FPGA.
    /// Place the riscv64 busybox at test/build/busybox.riscv64.
    pub static BUSYBOX: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../test/build/busybox.riscv64"
    ));

    #[allow(unsafe_code)]
    #[export_name = "__asterinas_busybox"]
    pub extern "Rust" fn busybox_blob() -> &'static [u8] {
        BUSYBOX
    }

    #[allow(unsafe_code)]
    #[export_name = "__asterinas_busybox_range"]
    pub extern "Rust" fn busybox_range() -> (*const u8, usize) {
        (BUSYBOX.as_ptr(), BUSYBOX.len())
    }
}

pub mod arch;
pub mod context;
pub mod cpu;
pub mod device;
pub mod driver;
pub mod error;
pub mod events;
pub mod fs;
pub mod ipc;
pub mod lock;
pub mod net;
pub mod prelude;
mod process;
mod sched;
mod threadlet_rpc;
#[cfg(target_arch = "riscv64")]
mod threadlet_scheduler;
#[cfg(target_arch = "riscv64")]
mod threadlet_syscall;
#[cfg(target_arch = "riscv64")]
mod threadlet_arch;
pub mod syscall;
pub mod thread;
pub mod time;
mod util;
pub(crate) mod vdso;
pub mod vm;

#[cfg(target_arch = "riscv64")]
#[repr(C, align(64))]
struct CacheLineAtomicU64 {
    value: AtomicU64,
    _pad: [u8; 64 - core::mem::size_of::<AtomicU64>()],
}

#[cfg(target_arch = "riscv64")]
impl CacheLineAtomicU64 {
    const fn new(v: u64) -> Self {
        Self {
            value: AtomicU64::new(v),
            _pad: [0u8; 64 - core::mem::size_of::<AtomicU64>()],
        }
    }
}

#[cfg(target_arch = "riscv64")]
static DCACHE_PROBE_TEST_X: CacheLineAtomicU64 = CacheLineAtomicU64::new(0);
#[cfg(target_arch = "riscv64")]
static DCACHE_PROBE_TEST_BSP_READY: AtomicU64 = AtomicU64::new(0);
#[cfg(target_arch = "riscv64")]
static DCACHE_PROBE_TEST_MONITOR_READY: AtomicU64 = AtomicU64::new(0);
#[cfg(target_arch = "riscv64")]
static DCACHE_PROBE_TEST_DONE: AtomicU64 = AtomicU64::new(0);
#[cfg(target_arch = "riscv64")]
#[repr(align(64))]
struct AlignedAtomicU64(AtomicU64);
#[cfg(target_arch = "riscv64")]
static DCACHE_PROBE_TEST_STORM: AlignedAtomicU64 = AlignedAtomicU64(AtomicU64::new(0));


const SCHED_TEST_ROLE_A: u64 = 0;
const SCHED_TEST_ROLE_B: u64 = 1;
const SCHED_TEST_ROLE_C: u64 = 2;
const SCHED_TEST_NOP_CYCLES: usize = 50_000;
const SCHED_TEST_A_PREWAKE_NOP_CYCLES: usize = 100;
static SCHED_TEST_THREADLET_B_HART: AtomicU64 = AtomicU64::new(u32::MAX as u64);
static SCHED_TEST_THREADLET_C_HART: AtomicU64 = AtomicU64::new(u32::MAX as u64);


/* For rpc tests */
#[cfg(target_arch = "riscv64")]
const PREEMPT_SLICE: u64 = 5120;
#[cfg(target_arch = "riscv64")]
static WORKER_THREADLET_START_ID: AtomicU64 = AtomicU64::new(u32::MAX as u64);
#[cfg(target_arch = "riscv64")]
const MAX_RPC_CNT_CPUS: usize = 64;
#[cfg(target_arch = "riscv64")]
static AP_IDLE_NOP_LOOP_CNT: [AtomicU64; MAX_RPC_CNT_CPUS] =
    [const { AtomicU64::new(0) }; MAX_RPC_CNT_CPUS];
#[cfg(target_arch = "riscv64")]
const THREADLET_CTX_THREADS_PER_CORE: usize = 512;
#[cfg(target_arch = "riscv64")]
const THREADLET_CTX_REGS: usize = 32;
#[cfg(target_arch = "riscv64")]
const THREADLET_CTX_REG_BYTES: usize = 8;
#[cfg(target_arch = "riscv64")]
const THREADLET_CTX_BYTES_PER_CORE: usize =
    THREADLET_CTX_THREADS_PER_CORE * THREADLET_CTX_REGS * THREADLET_CTX_REG_BYTES;
#[cfg(target_arch = "riscv64")]
// Keep this segment alive so the frames never return to the allocator.
static THREADLET_CTX_SEG: SpinOnce<Segment<()>> = SpinOnce::new();

#[cfg(target_arch = "riscv64")]
#[inline(always)]
fn ap_idle_nop_loop_cnt_inc(cpu_id: usize) {
        AP_IDLE_NOP_LOOP_CNT[cpu_id].fetch_add(1, Ordering::Relaxed);
}

#[cfg(target_arch = "riscv64")]
#[inline(always)]
pub(crate) fn ap_idle_nop_loop_cnt_read(cpu_id: usize) -> u64 {
    if cpu_id < MAX_RPC_CNT_CPUS {
        AP_IDLE_NOP_LOOP_CNT[cpu_id].load(Ordering::Relaxed)
    } else {
        0
    }
}



#[cfg(target_arch = "riscv64")]
pub(crate) fn kernel_vaddr_to_paddr(vaddr: usize) -> Option<usize> {
    // - The kernel image is mapped at `KERNEL_CODE_BASE_VADDR` (a fixed offset mapping).
    // - The heap allocator allocates pages from the linear mapping (direct map).
    // We only need a lightweight translation here to feed threadlet D$ monitor instructions.
    const KERNEL_CODE_BASE_VADDR: usize = 0xffff_ffff_0000_0000;
    const LINEAR_MAPPING_BASE_VADDR: usize = 0xffff_8000_0000_0000;

    if vaddr >= KERNEL_CODE_BASE_VADDR {
        vaddr.checked_sub(KERNEL_CODE_BASE_VADDR)
    } else if vaddr >= LINEAR_MAPPING_BASE_VADDR {
        vaddr.checked_sub(LINEAR_MAPPING_BASE_VADDR)
    } else {
        None
    }
}

#[cfg(target_arch = "riscv64")]
fn threadlet_ctx_linear_vaddr_from_paddr(paddr: usize) -> usize {
    const LINEAR_MAPPING_BASE_VADDR: usize = 0xffff_8000_0000_0000;
    paddr
        .checked_add(LINEAR_MAPPING_BASE_VADDR)
        .expect("threadlet ctx linear vaddr overflow")
}

#[cfg(target_arch = "riscv64")]
fn threadlet_ctx_base_paddr(cpu_id: usize) -> usize {
    let seg = THREADLET_CTX_SEG.call_once(|| {
        let cpus = ostd::cpu::num_cpus();
        assert!(cpus > 0, "threadlet ctx init before CPU discovery");
        let bytes = THREADLET_CTX_BYTES_PER_CORE
            .checked_mul(cpus)
            .expect("threadlet ctx area size overflow");
        let nframes = (bytes + PAGE_SIZE - 1) / PAGE_SIZE;
        FrameAllocOptions::new()
            .zeroed(true)
            .alloc_segment(nframes)
            .expect("alloc threadlet context save area failed")
    });

    let cpus = ostd::cpu::num_cpus();
    assert!(
        cpu_id < cpus,
        "threadlet ctx cpu_id={} exceeds num_cpus={}",
        cpu_id,
        cpus
    );
    let offset = cpu_id
        .checked_mul(THREADLET_CTX_BYTES_PER_CORE)
        .expect("threadlet ctx per-cpu offset overflow");
    let base_paddr = seg
        .start_paddr()
        .checked_add(offset)
        .expect("threadlet ctx base paddr overflow");
    let end_offset = offset
        .checked_add(THREADLET_CTX_BYTES_PER_CORE)
        .expect("threadlet ctx end offset overflow");
    assert!(
        end_offset <= seg.size(),
        "threadlet ctx area is smaller than the CPU-local slice"
    );
    assert_eq!(base_paddr % THREADLET_CTX_REG_BYTES, 0);

    let base_vaddr = threadlet_ctx_linear_vaddr_from_paddr(base_paddr);
    let checked_paddr =
        kernel_vaddr_to_paddr(base_vaddr).expect("threadlet ctx vaddr to paddr failed");
    assert_eq!(checked_paddr, base_paddr);
    base_paddr
}

#[cfg(target_arch = "riscv64")]
fn init_threadlet_ctx_base_on_current_core(cpu_id: CpuId) {
    let base_paddr = threadlet_ctx_base_paddr(cpu_id.as_usize());
    ostd::arch::riscv::threadlet::threadlet_set_ctx_base(base_paddr);
    ostd::early_println!(
        "[threadlet] context save area cpu={} base={:#x}",
        cpu_id.as_usize(),
        base_paddr
    );
}

#[cfg(target_arch = "riscv64")]
fn dcache_probe_test_bsp_stress_words() -> usize {
    // 128 KiB of 8-byte words = 2048 KiB, intentionally larger than typical L1D.
    128 * 1024
}

#[cfg(target_arch = "riscv64")]
fn dcache_probe_test_bsp_stress_rounds_per_check() -> usize {
    256
}

#[cfg(target_arch = "riscv64")]
fn dcache_probe_test_bsp_stress_enabled() -> bool {
    true
}

#[cfg(target_arch = "riscv64")]
fn dcache_probe_test_stress_step(buf: &Segment<()>, words: usize, iter: &mut usize) {
    let len = words;
    debug_assert!(len != 0);
    let i = *iter;
    let idx = i.wrapping_mul(17) % len;
    let off = idx * core::mem::size_of::<u64>();
    let v: u64 = buf.read_val(off).expect("stress read_val failed");
    let new_v = v.wrapping_add(1);
    buf.write_val(off, &new_v).expect("stress write_val failed");
    black_box(v);
    black_box(DCACHE_PROBE_TEST_STORM.0.load(Ordering::Relaxed));
    *iter = i.wrapping_add(1);
}

#[cfg(target_arch = "riscv64")]
fn dcache_probe_test_ap_delay_spins() -> usize {
    2000_00
}

#[cfg(target_arch = "riscv64")]
fn dcache_probe_test_ap_probe_storm_period() -> usize {
    128
}

#[cfg(target_arch = "riscv64")]
fn dcache_probe_test_enabled() -> bool {
    let Some(args) = kernel_cmdline().get_module_args("dcache_probe_test") else {
        return false;
    };
    for a in args {
        match a {
            ModuleArg::Arg(name) if name.as_bytes() == b"enable" => return true,
            ModuleArg::KeyVal(name, value) if name.as_bytes() == b"enable" => {
                if value.as_bytes() == b"1" || value.as_bytes() == b"true" {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}


#[cfg(target_arch = "riscv64")]
fn scheduling_test_enabled() -> bool {
    kernel_cmdline()
        .get_initproc_argv()
        .iter()
        .any(|a| a.as_bytes() == b"enable_scheduling_test")
}

#[cfg(target_arch = "riscv64")]
fn scheduling_test_nop_cycles(cycles: usize) {
    for _ in 0..cycles {
        #[allow(unsafe_code)]
        unsafe {
            core::arch::asm!("nop", options(nomem, nostack));
        }
    }
}

fn scheduling_test_threadlet_entry() {
    use ostd::arch::riscv::threadlet;

    match threadlet::threadlet_get_a0() {
        SCHED_TEST_ROLE_A => {
            threadlet::threadlet_syn_print(6, 0);
            // PASS with only A runnable: A should continue.
            threadlet::threadlet_pass();
            scheduling_test_nop_cycles(SCHED_TEST_A_PREWAKE_NOP_CYCLES);

            // Wake B, then PASS again: scheduler should switch to B.
            let b_hart = SCHED_TEST_THREADLET_B_HART.load(Ordering::Acquire) as u32;
            let c_hart = SCHED_TEST_THREADLET_C_HART.load(Ordering::Acquire) as u32;
            if b_hart != u32::MAX {
                threadlet::threadlet_syn_print(6, 1);
                threadlet::threadlet_wakeup(b_hart);
                threadlet::threadlet_wakeup(c_hart);
            }
            scheduling_test_nop_cycles(SCHED_TEST_A_PREWAKE_NOP_CYCLES);
            threadlet::threadlet_syn_print(6, 2);
            threadlet::threadlet_pass();

            scheduling_test_nop_cycles(SCHED_TEST_NOP_CYCLES);
            threadlet::threadlet_yield();
        }
        SCHED_TEST_ROLE_B => {
            // Keep the old marker point; C is woken explicitly by A.
            let c_hart = SCHED_TEST_THREADLET_C_HART.load(Ordering::Acquire) as u32;
            if c_hart != u32::MAX {
                threadlet::threadlet_syn_print(6, 3);
            }

            scheduling_test_nop_cycles(SCHED_TEST_NOP_CYCLES);
            threadlet::threadlet_yield();
        }
        SCHED_TEST_ROLE_C => {
            threadlet::threadlet_syn_print(6, 5);
            scheduling_test_nop_cycles(SCHED_TEST_NOP_CYCLES);
            threadlet::threadlet_yield();
        }
        _ => {}
    }

    loop {
        threadlet::threadlet_yield();
    }
}

#[cfg(target_arch = "riscv64")]
fn dcache_probe_waiter_threadlet() {
    use crate::net::socket::vsock::addr;
	// let addr = ostd::arch::riscv::threadlet::threadlet_get_a0() as usize;
    let init_val = DCACHE_PROBE_TEST_X.value.load(Ordering::Acquire);
    ostd::early_println!(
        "[dcache-probe-test][WAIT] start: X={}  mwait...",
        init_val,
    );
    ostd::arch::riscv::threadlet::threadlet_dcache_print_enable();
    DCACHE_PROBE_TEST_BSP_READY.store(1, Ordering::Release);

    while (true){
        ostd::arch::riscv::threadlet::threadlet_syn_print(5, 1);
        let vaddr = (&DCACHE_PROBE_TEST_X.value as *const AtomicU64) as usize;
        let paddr = kernel_vaddr_to_paddr(vaddr).expect("kernel_vaddr_to_paddr failed");
        ostd::arch::riscv::threadlet::threadlet_dcache_monitor_set(paddr,0);
        ostd::arch::riscv::threadlet::threadlet_syn_print(5, paddr as u64);
        let new_val = DCACHE_PROBE_TEST_X.value.load(Ordering::Acquire); // we must guarantee that the value is in the cacheline
        if new_val == 0 {
            ostd::arch::riscv::threadlet::threadlet_syn_print(5, 1);
            ostd::arch::riscv::threadlet::threadlet_yield();
            continue; // After waking up, the monitor bit is automatically cleared
        }
        else {
            ostd::arch::riscv::threadlet::threadlet_dcache_monitor_clear(paddr, 0);
            break;
        }
    }


    ostd::arch::riscv::threadlet::threadlet_syn_print(5, 1);
    let new_val = DCACHE_PROBE_TEST_X.value.load(Ordering::Acquire);
    
    ostd::early_println!("[dcache-probe-test][WAIT] woken: X={}", new_val);

    DCACHE_PROBE_TEST_DONE.store(1, Ordering::Release);
    ostd::arch::riscv::threadlet::threadlet_yield();
    panic!("monitor cleared, should not reach here");
}

#[ostd::main]
#[controlled]
pub fn main() {
    ostd::early_println!("[kernel] OSTD initialized. Preparing components.");
    component::init_all(component::parse_metadata!()).unwrap();
    init();

    // Spawn all AP idle threads.
    ostd::early_println!("[kernel] register_ap_entry.\n");
    ostd::boot::smp::register_ap_entry(ap_init);

    // Spawn the first kernel thread on BSP.
    let mut affinity = CpuSet::new_empty();
    affinity.add(CpuId::bsp());

    // //  create a threadlet
    // {
    //     use crate::thread::kernel_thread::ThreadOptions as KThreadOptions;
    //     let _ = KThreadOptions::threadlet_new_auto(idle_threadlet);
    // }

    ThreadOptions::new(init_thread)
        .priority(Priority::idle())
        .cpu_affinity(affinity)
        .spawn();
}

pub fn init() {
    util::random::init();
    driver::init();
    time::init();
    #[cfg(target_arch = "riscv64")]
    {
        aster_network::icenet::register_if_present();
    }
    // Enable network stack on all supported architectures (including RISC-V).
    net::init();
    sched::init();
    // Debug: print initramfs buffer length and magic before unpacking
    let initrd_buf = boot::initramfs();
    let head = initrd_buf.get(0..4).unwrap_or(&[]);
    println!(
        "[kernel] initramfs: len={} head={:02x?}",
        initrd_buf.len(),
        head
    );
    fs::rootfs::init(initrd_buf).unwrap();
    device::init().unwrap();
    syscall::init();
    vdso::init();
    process::init();
}

fn ap_init() {
	    fn ap_idle_thread() {
	        let preempt_guard = ostd::task::disable_preempt();
	        let cpu_id = preempt_guard.current_cpu();
	        drop(preempt_guard);
            let nop_time: usize = cpu_id.as_usize() * 1000000;

            for _i in 0..nop_time {
                #[allow(unsafe_code)]
                unsafe {
                    core::arch::asm!("nop", options(nomem, nostack));
                }
            }  

            ostd::arch::riscv::threadlet::threadlet_set_priority(0, 30);
	        ostd::early_println!("[kernel] Kernel idle thread for CPU #{} started.", cpu_id.as_usize());
            ostd::arch::riscv::threadlet::threadlet_set_priority(0, 1);


            let use_nop = ostd::arch::riscv::boot::smp::threadlet_present();
            if use_nop {
                loop {
                    for _ in 0..1000 {
                        #[allow(unsafe_code)]
                        unsafe {
                            core::arch::asm!("nop", options(nomem, nostack));
                        }
                    }
                    ap_idle_nop_loop_cnt_inc(cpu_id.as_usize());
                }
            } else {
                loop {
                    Thread::yield_now();
                }
            }
		}

    let preempt_guard = ostd::task::disable_preempt();
    let cpu_id = preempt_guard.current_cpu();
    drop(preempt_guard);

    let nop_time: usize = cpu_id.as_usize() * 1000000;
    for _i in 0..nop_time {
            #[allow(unsafe_code)]
		    unsafe {
		        core::arch::asm!("nop", options(nomem, nostack));
		    }
    }  


    ostd::early_println!("[kernel] AP initialized.\n");


    // {
    //     use crate::thread::kernel_thread::ThreadOptions as KThreadOptions;
    //     let _ = KThreadOptions::threadlet_new_auto(ap_idle_thread);
    // }
    if ostd::arch::boot::smp::threadlet_present(){

        ostd::arch::riscv::threadlet::threadlet_init_on_core();

        init_threadlet_ctx_base_on_current_core(cpu_id);
        ostd::arch::riscv::threadlet::threadlet_set_priority(0, 1);
    }
    if ostd::arch::boot::smp::intr_threadlet_present(){
        if (true)
        {   
            if !aster_network::icenet::threadlet_polling_enabled() {
	                    let _ = ThreadOptions::threadlet_new_auto_with_arg(
	                        icenet_bottom_half_threadlet,
	                        ICENET_SOFTIRQ_PRIO,
	                        0,
	                        false,
	                    );
	        }
            let val = ostd::arch::riscv::trap::trap_threadlet_vector_base as usize;
            ostd::arch::riscv::threadlet::enable_interrupt_threadlet_with_base(val);
            ostd::arch::riscv::threadlet::threadlet_syn_print(5,64);
            ostd::arch::riscv::threadlet::threadlet_syn_print(5,64);
        }
    }

	#[cfg(target_arch = "riscv64")]
	    {
	        if crate::lock::mcs_threadlet::enabled() {
	            let ctx_base_paddr = threadlet_ctx_base_paddr(cpu_id.as_usize());
	            crate::lock::mcs_threadlet::ap_run_workload_until_done(
	                cpu_id,
	                ctx_base_paddr,
	            );
	        }
	        // DCache probe test: wait for BSP to enter the spin loop, then write X from AP.
	        if dcache_probe_test_enabled()
                    && ostd::arch::riscv::boot::smp::threadlet_present()
            {
                let words = dcache_probe_test_bsp_stress_words();
                let mut iter: usize = 0;
                let stress_seg = if dcache_probe_test_bsp_stress_enabled() {
                    let bytes = words
                        .checked_mul(core::mem::size_of::<u64>())
                        .expect("stress bytes overflow");
                    let nframes = (bytes + PAGE_SIZE - 1) / PAGE_SIZE;
                    FrameAllocOptions::new()
                        .alloc_segment(nframes)
                        .expect("alloc stress segment failed")
                } else {
                    FrameAllocOptions::new()
                        .alloc_segment(1)
                        .expect("alloc minimal stress segment failed")
                };
                let stress_words = stress_seg.size() / core::mem::size_of::<u64>();
                debug_assert!(stress_words != 0);

                while DCACHE_PROBE_TEST_BSP_READY.load(Ordering::Acquire) == 0 {
                    #[allow(unsafe_code)]
		            unsafe {
		                        core::arch::asm!("nop", options(nomem, nostack));
		            }
                }
                if dcache_probe_test_bsp_stress_enabled() {
                    for i in 0..dcache_probe_test_ap_delay_spins() {
                        dcache_probe_test_stress_step(&stress_seg, stress_words, &mut iter);
                        #[allow(unsafe_code)]
		                unsafe {
		                        core::arch::asm!("nop", options(nomem, nostack));
		                }
                    }
                }
                else{
                    for i in 0..(10*dcache_probe_test_ap_delay_spins()) {
                        #[allow(unsafe_code)]
		                unsafe {
		                        core::arch::asm!("nop", options(nomem, nostack));
		                }
                    }
                }

                ostd::arch::riscv::threadlet::threadlet_dcache_print_enable();
                ostd::arch::riscv::threadlet::threadlet_syn_print(5, 0);
                DCACHE_PROBE_TEST_X.value.store(1, Ordering::Release);
                for _i in 0..800000 {
                    #[allow(unsafe_code)]
		            unsafe {
		                        core::arch::asm!("nop", options(nomem, nostack));
		            }
                }       
                ostd::arch::riscv::threadlet::threadlet_dcache_print_disable();
            }

            // Spawn AP service threadlets according to mode.
            if ostd::arch::riscv::boot::smp::threadlet_present() {
                for _i in 0..1000000* cpu_id.as_usize() {
                        #[allow(unsafe_code)]
		                unsafe {
		                        core::arch::asm!("nop", options(nomem, nostack));
		                }
                }

                if !aster_network::icenet::threadlet_polling_enabled() {
                } else if cpu_id.as_usize() >= aster_network::icenet::WORKER_CORE_ID
                    && cpu_id.as_usize() < aster_network::icenet::WORKER_CORE_ID
                            + {
                                aster_network::icenet::init_poll_rpc_worker_topology();
                                aster_network::icenet::poll_rpc_worker_core_count()
                            }
	                {
	                    let worker_core_idx =
	                        cpu_id.as_usize() - aster_network::icenet::WORKER_CORE_ID;
	                    if worker_core_idx == 0 {
	                        WORKER_THREADLET_START_ID.store(u32::MAX as u64, Ordering::Release);
	                    }
	                    let ctx_base_paddr = threadlet_ctx_base_paddr(cpu_id.as_usize());
	                    let mut worker_harts =
	                        [u32::MAX; aster_network::icenet::NUM_RPC_WORKER_THREADLET];
	                    for worker_idx in 0..aster_network::icenet::NUM_RPC_WORKER_THREADLET {
	                        let worker_global_idx = worker_core_idx
	                            * aster_network::icenet::NUM_RPC_WORKER_THREADLET
	                            + worker_idx;
	                        let (_t, hart_id) = ThreadOptions::threadlet_new_auto_with_arg_with_mem(
	                            threadlet_rpc::worker_threadlet,
	                            threadlet_rpc::RPC_WORKER_THREADLET_PRIO,
	                            worker_global_idx as u64,
	                            false,
	                            ctx_base_paddr,
	                        );
	                        if hart_id == u32::MAX {
	                            ostd::arch::riscv::threadlet::threadlet_set_priority(0, 30);
	                            ostd::early_println!("[rpc-worker] create failed!");
	                            ostd::arch::riscv::threadlet::threadlet_set_priority(0, 1);
	                            continue;
	                        }

	                        if worker_core_idx == 0 && worker_idx == 0 {
	                            WORKER_THREADLET_START_ID.store(hart_id as u64, Ordering::Release);
	                        }
	                        ostd::arch::riscv::threadlet::threadlet_set_timeslice(hart_id, PREEMPT_SLICE);
	                        worker_harts[worker_idx] = hart_id;
	                    }
	                    ostd::arch::riscv::threadlet::threadlet_set_priority(0, 30);
	                    for &hart_id in worker_harts.iter() {
	                        if hart_id != u32::MAX {
	                            ostd::arch::riscv::threadlet::threadlet_wakeup(hart_id);
	                        }
	                    }
	                    ostd::early_println!("--------------All rpc_workers init finished on cpu={}",cpu_id.as_usize());
	                    ostd::arch::riscv::threadlet::threadlet_set_priority(0, 1);
	                }

                if syscall_delegation_enabled() && cpu_id.as_usize() == 1 {
                    if !syscall_delegation_wait_bsp_ready(SYSCALL_DELEG_READY_WAIT_SPINS) {
                        ostd::early_println!(
                            "[syscall-deleg][ap{}] timeout waiting for BSP handler",
                            cpu_id.as_usize()
                        );
                    } else {
                        let (_t, hart_id) = ThreadOptions::threadlet_new_auto_with_arg(
                            syscall_delegation_producer_threadlet,
                            SYSCALL_DELEG_PRODUCER_PRIO,
                            0,
                            false,
                        );
                        if hart_id == u32::MAX {
                            ostd::early_println!(
                                "[syscall-deleg][ap{}] create producer threadlet failed",
                                cpu_id.as_usize()
                            );
                        } else {
                            ostd::arch::riscv::threadlet::threadlet_set_priority(0, 30);
                            ostd::early_println!(
                                "[syscall-deleg][ap{}] producer threadlet created: hart={}",
                                cpu_id.as_usize(),
                                hart_id
                            );
                            ostd::arch::riscv::threadlet::threadlet_set_priority(0, 1);
                            ostd::arch::riscv::threadlet::threadlet_wakeup(hart_id);

                        }
                    }
                }
            }
            //else the bsp will create the normal udp waiter for ap cores.
	    }



    ThreadOptions::new(ap_idle_thread)
        .cpu_affinity(cpu_id.into())
        .priority(Priority::idle())
        .spawn();
}

fn init_thread() {
    println!("[kernel] Spawn init thread");
    // Work queue should be initialized before interrupt is enabled,
    // in case any irq handler uses work queue as bottom half
    thread::work_queue::init();
    // Enable network stack background workers on all supported architectures (including RISC-V).
    net::lazy_init();
    fs::lazy_init();
    ipc::init();
    // driver::pci::virtio::block::block_device_test();
    let thread = ThreadOptions::new(|| {
        println!("[kernel] Hello world from kernel!");

    })
    .spawn();
    thread.join();

    print_banner();

    let karg = boot::kernel_cmdline();

    // Extra debug to observe initproc path/argv/envp on real hardware.
    ostd::early_println!(
        "[init] initproc path={:?} argv_len={} envp_len={}",
        karg.get_initproc_path(),
        karg.get_initproc_argv().len(),
        karg.get_initproc_envp().len()
    );
    info!("Init process path: {:?}", karg.get_initproc_path());

    do_nop_cycle_test();

    // dcache-mwait test
    #[cfg(target_arch = "riscv64")]
    {
        let preempt_guard = ostd::task::disable_preempt();
        let cpu_id = preempt_guard.current_cpu();
        drop(preempt_guard);


        if ostd::arch::riscv::boot::smp::threadlet_present() {
            ostd::arch::riscv::threadlet::threadlet_init_on_core();
                    
            init_threadlet_ctx_base_on_current_core(cpu_id);
            ostd::arch::riscv::threadlet::threadlet_set_priority(0, 1);    

        }

        if ostd::arch::riscv::boot::smp::intr_threadlet_present(){
            if (true)
            {
                if !aster_network::icenet::threadlet_polling_enabled() {
                    let (_t_icenet, icenet_hart) =
                        ThreadOptions::threadlet_new_auto_with_arg(icenet_bottom_half_threadlet, ICENET_SOFTIRQ_PRIO, 0, false);
                    if icenet_hart != u32::MAX {
                        aster_network::icenet::set_icenet_bh_threadlet_hart(icenet_hart);
                    }
                }

                let val = ostd::arch::riscv::trap::trap_threadlet_vector_base as usize;
                ostd::arch::riscv::threadlet::enable_interrupt_threadlet_with_base(val);
                ostd::arch::riscv::threadlet::threadlet_syn_print(5,64);
                ostd::arch::riscv::threadlet::threadlet_syn_print(5,64);
            }
        }

        if ostd::arch::riscv::boot::smp::threadlet_present() {
            if threadlet_arch::ctx_saver_test_enabled() {
                    let ctx_base_paddr = threadlet_ctx_base_paddr(cpu_id.as_usize());
                    threadlet_arch::run_ctx_saver_test(cpu_id, ctx_base_paddr);
            }
        }


        // mcslock test
        if cpu_id == CpuId::bsp() && crate::lock::mcs_threadlet::enabled() {
            let ctx_base_paddr = threadlet_ctx_base_paddr(cpu_id.as_usize());
            crate::lock::mcs_threadlet::bsp_run_workload_until_done(
                cpu_id,
                ctx_base_paddr,
            );
        }

        if dcache_probe_test_enabled() && cpu_id == CpuId::bsp() {
            let vaddr = (&DCACHE_PROBE_TEST_X.value as *const AtomicU64) as usize;
            let paddr = kernel_vaddr_to_paddr(vaddr).expect("kernel_vaddr_to_paddr failed");
            let addr = paddr as u64;
            let storm_vaddr = (&DCACHE_PROBE_TEST_STORM as *const AlignedAtomicU64) as usize;
            let storm_paddr = kernel_vaddr_to_paddr(storm_vaddr).unwrap_or(0);
            let done_vaddr = (&DCACHE_PROBE_TEST_DONE as *const AtomicU64) as usize;
            let done_paddr = kernel_vaddr_to_paddr(done_vaddr).unwrap_or(0);
            let init_val = DCACHE_PROBE_TEST_X.value.load(Ordering::Acquire);
            ostd::early_println!("---------------------------");
            ostd::early_println!(
                "----------------------[dcache-probe-test][BSP] init: X={} (vaddr={:#x} paddr={:#x}), spawn waiter...",
                init_val,
                vaddr,
                addr,
            );
            if dcache_probe_test_bsp_stress_enabled() {
                ostd::early_println!(
                    "[dcache-probe-test][BSP] addrs: storm(vaddr={:#x} paddr={:#x}) done(vaddr={:#x} paddr={:#x})",
                    storm_vaddr,
                    storm_paddr,
                    done_vaddr,
                    done_paddr
                );
            }

            let words = dcache_probe_test_bsp_stress_words();
            let mut iter: usize = 0;
            let stress_seg = if dcache_probe_test_bsp_stress_enabled() {
                let bytes = words
                    .checked_mul(core::mem::size_of::<u64>())
                    .expect("stress bytes overflow");
                let nframes = (bytes + PAGE_SIZE - 1) / PAGE_SIZE;
                FrameAllocOptions::new()
                    .alloc_segment(nframes)
                    .expect("alloc stress segment failed")
            } else {
                FrameAllocOptions::new()
                    .alloc_segment(1)
                    .expect("alloc minimal stress segment failed")
            };
            let stress_words = stress_seg.size() / core::mem::size_of::<u64>();
            debug_assert!(stress_words != 0);

            let (_t_waiter, waiter_hart) = ThreadOptions::threadlet_new_auto_with_arg(
                    dcache_probe_waiter_threadlet,
                    8,
                    addr,
                    true,
            );
            for _ in 0..100 {
                ostd::arch::riscv::threadlet::threadlet_syn_print(5,2);
            }

            if dcache_probe_test_bsp_stress_enabled() {
                while DCACHE_PROBE_TEST_DONE.load(Ordering::Acquire) == 0 {
                    for _ in 0..dcache_probe_test_bsp_stress_rounds_per_check() {
                        dcache_probe_test_stress_step(&stress_seg, stress_words, &mut iter);
                    }
                    ostd::arch::riscv::threadlet::threadlet_syn_print(5,3);
                    
                }
            } else {
                while DCACHE_PROBE_TEST_DONE.load(Ordering::Acquire) == 0 {
                    #[allow(unsafe_code)]
		            unsafe {
		                        core::arch::asm!("nop", options(nomem, nostack));
		            }
                }
            }
            ostd::arch::riscv::threadlet::threadlet_syn_print(5,3);
            ostd::arch::riscv::threadlet::threadlet_dcache_print_disable();
        }
	}
    
    // Spawn kernel UDP waiter threads or hardware-managed threadlets to block on socket I/O.
    #[cfg(target_arch = "riscv64")]
    {
        for _i in 0..9000000 {
            #[allow(unsafe_code)]
		    unsafe {
		        core::arch::asm!("nop", options(nomem, nostack));
		    }
        }   
        if ostd::arch::riscv::boot::smp::threadlet_present() {
            if aster_network::icenet::threadlet_udp_waiter_enabled() {
                ostd::early_println!("-------------------------[init] spawning kernel UDP waiters/threadlets");
                let (_t1, _hart1) =
                    ThreadOptions::threadlet_new_auto_with_arg(kernel_udp_threadlet_waiter, 3, 5555, true);
            }
        } else {
            ThreadOptions::new(|| kernel_udp_waiter(5555))
                .cpu_affinity(CpuId::bsp().into())
                .spawn();
        }
    }



    if ostd::arch::boot::smp::intr_threadlet_present(){
        // if it is the threadlet_polling mode, create a threadlet to poll
        if aster_network::icenet::threadlet_polling_enabled() {
            // Spawn one polling threadlet per RX queue (q is passed via a0).
            // spawn q=0 for bsp, and q>0 for all aps.
            let _ = ThreadOptions::threadlet_new_auto_with_arg(
                    icenet_polling_threadlet,
                    ICENET_SOFTIRQ_PRIO,
                    0 as u64,
                    true,
            );
        }
    }


    // If device tree reports threadlet scheduling tests "enable_scheduling_test"
    #[cfg(target_arch = "riscv64")]
    {
        if scheduling_test_enabled() {
            if ostd::arch::riscv::boot::smp::threadlet_present() {
                let preempt_guard = ostd::task::disable_preempt();
                let cpu_id = preempt_guard.current_cpu();
                drop(preempt_guard);
                let ctx_base_paddr = threadlet_ctx_base_paddr(cpu_id.as_usize());
                threadlet_scheduler::start_eevdf_scheduling_test(
                    cpu_id.as_usize(),
                    ctx_base_paddr,
                );
            } else {
                ostd::early_println!(
                    "[eevdf] scheduling_test enabled but hardware threadlet is not present"
                );
            }
        }
    }

    #[cfg(target_arch = "riscv64")]
    {
        if syscall_delegation_enabled() {
            if !ostd::arch::riscv::boot::smp::threadlet_present() {
                ostd::early_println!(
                    "[syscall-deleg][bsp] syscall_delegation set but threadlet not present"
                );
            } else {
                let seg = syscall_delegation_init_shared_buffer();
                debug_assert!(seg.size() >= SYSCALL_SHARED_BUF_BYTES);
                SYSCALL_DELEG_RING_HEAD.value.store(0, Ordering::Relaxed);
                SYSCALL_DELEG_RING_TAIL.value.store(0, Ordering::Relaxed);

                let (_t, hart_id) = ThreadOptions::threadlet_new_auto_with_arg(
                    syscall_delegation_handler_threadlet,
                    SYSCALL_DELEG_HANDLER_PRIO,
                    0,
                    false,
                );
                if hart_id == u32::MAX {
                    ostd::early_println!("[syscall-deleg][bsp] create handler threadlet failed");
                } else {
                    ostd::arch::riscv::threadlet::threadlet_set_priority(0, 30);
                    ostd::early_println!(
                        "[syscall-deleg][bsp] handler threadlet created: hart={}",
                        hart_id
                    );
                    ostd::arch::riscv::threadlet::threadlet_set_priority(0, 1);
                    SYSCALL_DELEG_HANDLER_HART.store(hart_id as u64, Ordering::Release);
                    ostd::arch::riscv::threadlet::threadlet_wakeup(hart_id);
                }
            }
        }
    }

  

    // If device tree reports hardware threadlets, run a simple threadlet test.
    //run_simple_threadlet_test();


    for _i in 0..9000000 {
        #[allow(unsafe_code)]
        unsafe {
            core::arch::asm!("nop", options(nomem, nostack));
        }
    }  

    // If hardware threadlet is detected from FDT, suppress launching userspace threads. We current do not support compressed instructions.
    #[cfg(target_arch = "riscv64")]
    {
        let init_path_opt = karg.get_initproc_path();
        // if ostd::arch::riscv::boot::smp::threadlet_present()
        if init_path_opt
                .map(|p| p.ends_with("/busybox") || p.ends_with("bin/busybox") || p.contains("busybox"))
                .unwrap_or(false)
        {

	        let use_nop = ostd::arch::riscv::boot::smp::threadlet_present();
	        if use_nop {
                ostd::arch::riscv::threadlet::threadlet_set_priority(0, 30);
                ostd::early_println!(
                    "[init] suppress launching busybox (init='{:?}')",
                    init_path_opt
                );
                ostd::arch::riscv::threadlet::threadlet_set_priority(0, 1);
		        loop {
		                    #[allow(unsafe_code)]
		                    unsafe {
		                        core::arch::asm!("nop", options(nomem, nostack));
		                    }
		        } 
		    } 
		    else {
                ostd::early_println!(
                    "[init] suppress launching busybox (init='{:?}')",
                    init_path_opt
                );
	            loop {
	                    Thread::yield_now();
	            }
	        }
        }
    }

    let initproc = Process::spawn_user_process(
        karg.get_initproc_path().unwrap(),
        karg.get_initproc_argv().to_vec(),
        karg.get_initproc_envp().to_vec(),
    )
    .expect("Run init process failed.");
    // Wait until the init process becomes a zombie (i.e., all threads exited).
    while !initproc.status().is_zombie() {
        // Cooperative yield to let other tasks run.
        Thread::yield_now();
    }

    // Read the init process' exit code and exit the machine accordingly.
    let code = initproc.status().exit_code();
    ostd::early_println!(
        "[kernel] init process exited: code={} — shutting down",
        code
    );
    if code == 0 {
        exit_qemu(QemuExitCode::Success);
    } else {
        exit_qemu(QemuExitCode::Failed);
    }
}

fn print_banner() {
    println!("\x1B[36m");
    println!(
        r"
 _____ _  _ ___  ___  _   ___  _    ___ _____
|_   _| || | _ \| __| /_\ |   \| |  | __|_   _|
  | | | __ |   /| _| / _ \| |) | |__| _|  | |
  |_| |_||_|_|_\|___/_/ \_\___/|____|___| |_|
"
    );
    println!("\x1B[0m");
}


/***********************************************************************************/
// Test Threads for Threadlet Network Stack

fn idle_threadlet() {
    while(true){}
}

#[cfg(target_arch = "riscv64")]
fn icenet_bottom_half_threadlet() {
    use aster_network::icenet::{self, NUM_CORES};
    use ostd::arch::riscv::threadlet;
    loop {
        // Always fetch the queue id from the current threadlet's a0 register.
        let q = threadlet::threadlet_get_a0() as usize;
        if q < NUM_CORES {
            threadlet::threadlet_syn_print(5, 130);
            icenet::icenet_softirq_handler_for_queue(q);
        }
        // Yield so that hardware can schedule other threadlets until we are
        // explicitly woken up again by the offload wakeup hook.
        threadlet::threadlet_yield();
    }
}

#[cfg(target_arch = "riscv64")]
pub const RPC_STAGE: u64 = 7;
#[allow(unsafe_code)]
fn icenet_polling_threadlet() {
    use ostd::arch::riscv::threadlet;

    const DOORBELL_CACHELINE_BYTES: usize = 64;

    // Queue id is fixed in a0 for this polling threadlet.
    let q = threadlet::threadlet_get_a0() as usize;
    if q >= aster_network::icenet::NUM_CORES {
        panic!("icenet_polling_threadlet: invalid queue id {}", q);
    }

    // Cache the polling doorbell base address and local pointer once to minimize overhead.
    let base_paddr = loop {
        if let Some(p) = aster_network::icenet::polling_doorbell_paddr(0) {
            break p as usize;
        }
        threadlet::threadlet_yield();
    };
    debug_assert_eq!(
        base_paddr & (DOORBELL_CACHELINE_BYTES - 1),
        0,
        "icenet doorbell base must be 64B-aligned"
    );

    let mmio = loop {
        if let Some(m) = ostd::arch::riscv::device::icenet::get_mmio() {
            break m;
        }
        threadlet::threadlet_yield();
    };

    let doorbell_paddr = base_paddr + q * DOORBELL_CACHELINE_BYTES;
    debug_assert_eq!(
        doorbell_paddr & (DOORBELL_CACHELINE_BYTES - 1),
        0,
        "icenet doorbell paddr must be 64B-aligned"
    );
    // Fast paddr->vaddr translation for linear mapping (direct map).
    const LINEAR_MAPPING_BASE_VADDR: usize = 0xffff_8000_0000_0000;
    let doorbell_vaddr = base_paddr + LINEAR_MAPPING_BASE_VADDR + q * DOORBELL_CACHELINE_BYTES;
    debug_assert_eq!(
        doorbell_vaddr & (DOORBELL_CACHELINE_BYTES - 1),
        0,
        "icenet doorbell vaddr must be 64B-aligned"
    );

    let tid:u32 = threadlet::threadlet_current() as u32;
    ostd::arch::riscv::threadlet::threadlet_set_priority(tid, 30);
    ostd::early_println!("--------------------[icenet-poll] doorbell_vaddr: {:#x}",doorbell_vaddr);
    ostd::arch::riscv::threadlet::threadlet_set_priority(tid, ICENET_SOFTIRQ_PRIO);
    let doorbell = unsafe { &*(doorbell_vaddr as *const AtomicU64) };
    let comp_cnt_off = 0xD0 + (q * 2) + 1;
    let mut poll_fast_ctx = loop {
        if let Some(ctx) = aster_network::icenet::polling_fast_path_init(q) {
            break ctx;
        }
        ostd::early_println!("[icenet-poll] polling_fast_path_init failed");
        threadlet::threadlet_yield();
    };

    loop {
        // Wait until the per-queue doorbell becomes 1.
        // Fast path: check first to avoid arming the monitor when already signaled.
        loop{
            threadlet::threadlet_dcache_monitor_set(doorbell_paddr, 0);
            threadlet::threadlet_syn_print(RPC_STAGE, 1);
            let comp_cnt: u8 = mmio.read_once(comp_cnt_off).unwrap_or(0);
            if (doorbell.load(Ordering::Relaxed) & 1) == 0 && comp_cnt == 0 {
                    threadlet::threadlet_syn_print(RPC_STAGE , 2);
                    threadlet::threadlet_yield();
                    threadlet::threadlet_syn_print(RPC_STAGE ,3);
                    continue;
            }
            threadlet::threadlet_dcache_monitor_clear(doorbell_paddr, 0);
            break;
        }


        // Minimal RX path: drain completions and print packets, no protocol stack.
        threadlet::threadlet_syn_print(RPC_STAGE,4);
        aster_network::icenet::polling_fast_path_process(&mut poll_fast_ctx);
    }
}

#[cfg(target_arch = "riscv64")]
fn syscall_delegation_producer_threadlet() {
    use ostd::arch::riscv::threadlet;

    let seg = loop {
        if let Some(seg) = SYSCALL_DELEG_SHARED_BUF.get() {
            break seg;
        }
        threadlet::threadlet_yield();
    };

    let dst_ip_be = u32::from_be_bytes(SYSCALL_TEST_DST_IP);
    let dst_port_be = SYSCALL_TEST_DST_PORT.to_be();
    let mut payload = [0u8; SYSCALL_SLOT_DATA_BYTES];

    for seq in 0..SYSCALL_TEST_SEND_COUNT {
        let payload_len = syscall_delegation_fill_payload(seq, &mut payload);
        let buf_slot = seq % MAX_SYSCALL_RING_SLOTS;
        let buf_off = buf_slot * SYSCALL_SLOT_DATA_BYTES;

        let mut writer = seg.writer().skip(buf_off).limit(payload_len);
        let written = writer.write(&mut VmReader::from(&payload[..payload_len]));
        if written != payload_len {
            ostd::early_println!(
                "[syscall-deleg][producer] write payload failed: seq={} written={} expected={}",
                seq,
                written,
                payload_len
            );
            continue;
        }

        let result = threadlet_user_send_to(
            seq as u32,
            buf_off as u32,
            payload_len as u32,
            dst_ip_be,
            dst_port_be,
        );
        if result != SYSCALL_COMPLETION_OK {
            ostd::early_println!(
                "[syscall-deleg][producer] sendto request failed: seq={} result={}",
                seq,
                result
            );
        }
    }

    loop {
        threadlet::threadlet_yield();
    }
}

#[cfg(target_arch = "riscv64")]
fn syscall_delegation_handler_threadlet() {
    use ostd::arch::riscv::threadlet;

    let seg = syscall_delegation_init_shared_buffer();
    let head_vaddr = (&SYSCALL_DELEG_RING_HEAD.value as *const AtomicU64) as usize;
    let head_paddr = kernel_vaddr_to_paddr(head_vaddr);

    let sock = crate::net::socket::ip::datagram::DatagramSocket::new(false);
    let mut packet = [0u8; SYSCALL_SLOT_DATA_BYTES];

    if head_paddr.is_none() {
        ostd::early_println!("[syscall-deleg][handler] head vaddr->paddr failed, fallback yield");
    }


    SYSCALL_DELEG_BSP_READY.store(1, Ordering::Release);

    loop {
        while let Some((slot_idx, req)) = syscall_deleg_try_dequeue() {
            if req.id != SYSCALL_REQ_ID_SENDTO {
                syscall_deleg_slot_store_completion(slot_idx, SYSCALL_COMPLETION_ERR);
                continue;
            }
            ostd::arch::riscv::threadlet::threadlet_syn_print(SYSCALL_STAGE, 100);

            let buf_off = req.buf_off as usize;
            let buf_len = req.buf_len as usize;
            let in_bounds = buf_len != 0
                && buf_len <= SYSCALL_SLOT_DATA_BYTES
                && buf_off
                    .checked_add(buf_len)
                    .map(|end| end <= SYSCALL_SHARED_BUF_BYTES)
                    .unwrap_or(false);
            if !in_bounds {
                syscall_deleg_slot_store_completion(slot_idx, SYSCALL_COMPLETION_ERR);
                continue;
            }

            let mut reader = seg.reader().skip(buf_off).limit(buf_len);
            let read = reader.read(&mut VmWriter::from(&mut packet[..buf_len]));
            if read != buf_len {
                syscall_deleg_slot_store_completion(slot_idx, SYSCALL_COMPLETION_ERR);
                continue;
            }

            let ip = req.dst_ipv4_be.to_be_bytes();
            let addr = SocketAddr::IPv4(
                Ipv4Address::new(ip[0], ip[1], ip[2], ip[3]),
                u16::from_be(req.dst_port_be),
            );
            let mh = MessageHeader::new(Some(addr), None);
            let mut payload_reader = VmReader::from(&packet[..buf_len]).to_fallible();
            match sock.sendmsg(&mut payload_reader, mh, SendRecvFlags::empty()) {
                Ok(_sent) => {
                    ostd::arch::riscv::threadlet::threadlet_syn_print(SYSCALL_STAGE, 101);
                    syscall_deleg_slot_store_completion(slot_idx, SYSCALL_COMPLETION_OK);
                }
                Err(e) => {
                    syscall_deleg_slot_store_completion(slot_idx, SYSCALL_COMPLETION_ERR);
                }
            }
        }

        if let Some(head_paddr) = head_paddr {
            loop {
                threadlet::threadlet_dcache_monitor_set(head_paddr, 0);
                threadlet::threadlet_syn_print(SYSCALL_STAGE, 1);
                if syscall_deleg_ring_empty() {
                    threadlet::threadlet_syn_print(SYSCALL_STAGE, 2);
                    threadlet::threadlet_yield();
                    continue;
                }
                threadlet::threadlet_dcache_monitor_clear(head_paddr, 0);
                break;
            }
        } else {
            threadlet::threadlet_yield();
        }
    }
}

#[cfg(target_arch = "riscv64")]
fn kernel_udp_threadlet_waiter() {
    use crate::net::socket::Socket;

    let arg = ostd::arch::riscv::threadlet::threadlet_get_a0() as usize;
    let port = (arg & 0xFFFF) as u16;
    ostd::early_println!("[kudp-threadlet] starting on port {}", port);

    // Determine bind IP via icenet.addr from kernel cmdline; fallback to FPGA default 172.16.0.2
    let mut ip = Ipv4Address::new(172, 16, 0, 2);
    if let Some(args) = kernel_cmdline().get_module_args("icenet") {
        for a in args {
            if let ModuleArg::KeyVal(name, value) = a {
                if name.as_bytes() == b"addr" {
                    if let Ok(s) = core::str::from_utf8(value.as_bytes()) {
                        let mut it = s.split('/');
                        if let Some(ip_s) = it.next() {
                            let parts: alloc::vec::Vec<&str> = ip_s.split('.').collect();
                            if parts.len() == 4 {
                                if let (Ok(a), Ok(b), Ok(c), Ok(d)) = (
                                    parts[0].parse::<u8>(),
                                    parts[1].parse::<u8>(),
                                    parts[2].parse::<u8>(),
                                    parts[3].parse::<u8>(),
                                ) {
                                    ip = Ipv4Address::new(a, b, c, d);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    let sock = crate::net::socket::ip::datagram::DatagramSocket::new(false);

    let addr = SocketAddr::IPv4(ip, port);
    match sock.bind(addr) {
        Ok(()) => {
            ostd::early_println!("[kudp-threadlet] bound UDP {}:{}", ip, port);
        }
        Err(e) => {
            ostd::early_println!(
                "[kudp-threadlet] bind failed {}:{} err={:?}",
                ip,
                port,
                e
            );
            return;
        }
    }

    // Log CPU and threadlet id where this waiter runs.
    let preempt_guard = ostd::task::disable_preempt();
    let cpu_id = preempt_guard.current_cpu();
    drop(preempt_guard);
    let tid = ostd::arch::riscv::threadlet::threadlet_current();
    ostd::early_println!(
        "[kudp-threadlet] started (listening on {}:{}) on CPU {} threadlet_id={} arg={:#x}",
        ip,
        port,
        cpu_id.as_usize(),
        tid,
        arg
    );

    let mut buf = [0u8; 2048];
    loop {
        let mut writer = VmWriter::from(&mut buf[..]).to_fallible();
        match sock.recvmsg(&mut writer, SendRecvFlags::empty()) {
            Ok((n, mh)) => {    
                ostd::arch::riscv::threadlet::threadlet_syn_print(5, 16);
                let show = core::cmp::min(n, 64);
                for i in 0..show {
                    if !(0x20..=0x7e).contains(&buf[i]) {
                        buf[i] = b'.';
                    }
                }
                let preview = core::str::from_utf8(&buf[..show]).unwrap_or("");
                let preempt_guard = ostd::task::disable_preempt();
                let cpu_id = preempt_guard.current_cpu();
                drop(preempt_guard);
                ostd::early_println!(
                    "[kudp-threadlet][cpu{}] I/O wakeup: recv {} bytes from {:?} preview='{}'",
                    cpu_id.as_usize(),
                    n,
                    mh.addr(),
                    preview
                );
            }
            Err(e) => {
                ostd::early_println!("[kudp-threadlet] recv error: {:?}", e);
                break;
            }
        }
    }
}

fn kernel_udp_waiter(port: u16) {
    // Determine bind IP via icenet.addr from kernel cmdline; fallback to FPGA default 172.16.0.2
    let mut ip = Ipv4Address::new(172, 16, 0, 2);
    if let Some(args) = kernel_cmdline().get_module_args("icenet") {
        for a in args {
            if let ModuleArg::KeyVal(name, value) = a {
                if name.as_bytes() == b"addr" {
                    if let Ok(s) = core::str::from_utf8(value.as_bytes()) {
                        let mut it = s.split('/');
                        if let Some(ip_s) = it.next() {
                            let parts: alloc::vec::Vec<&str> = ip_s.split('.').collect();
                            if parts.len() == 4 {
                                if let (Ok(a), Ok(b), Ok(c), Ok(d)) = (
                                    parts[0].parse::<u8>(),
                                    parts[1].parse::<u8>(),
                                    parts[2].parse::<u8>(),
                                    parts[3].parse::<u8>(),
                                ) {
                                    ip = Ipv4Address::new(a, b, c, d);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    let sock = crate::net::socket::ip::datagram::DatagramSocket::new(false);
    let addr = SocketAddr::IPv4(ip, port);
    match sock.bind(addr) {
        Ok(()) => {
            ostd::early_println!("[kudp] bound UDP {}:{}", ip, port);
        }
        Err(e) => {
            ostd::early_println!("[kudp] bind failed {}:{} err={:?}", ip, port, e);
            return;
        }
    }

    // Log CPU where this waiter runs
    let preempt_guard = ostd::task::disable_preempt();
    let cpu_id = preempt_guard.current_cpu();
    drop(preempt_guard);
    ostd::early_println!(
        "[kudp] started (listening on {}:{}) on CPU {}",
        ip,
        port,
        cpu_id.as_usize()
    );

    let mut buf = [0u8; 2048];
    loop {
        let mut writer = VmWriter::from(&mut buf[..]).to_fallible();
        match sock.recvmsg(&mut writer, SendRecvFlags::empty()) {
            Ok((n, mh)) => {
                ostd::arch::riscv::threadlet::threadlet_syn_print(5, 16);
                let show = core::cmp::min(n, 64);
                for i in 0..show { if !(0x20..=0x7e).contains(&buf[i]) { buf[i] = b'.'; } }
                let preview = core::str::from_utf8(&buf[..show]).unwrap_or("");
                let preempt_guard = ostd::task::disable_preempt();
                let cpu_id = preempt_guard.current_cpu();
                drop(preempt_guard);
                ostd::early_println!(
                    "[kudp][cpu{}] I/O wakeup: recv {} bytes from {:?} preview='{}'",
                    cpu_id.as_usize(),
                    n,
                    mh.addr(),
                    preview
                );
            }
            Err(e) => {
                ostd::early_println!("[kudp] recv error: {:?}", e);
                break;
            }
        }
    }
}

fn run_simple_threadlet_test() {
    #[cfg(target_arch = "riscv64")]
    {
        if ostd::arch::riscv::boot::smp::threadlet_present() {
            use ostd::arch::threadlet::threadlet_wakeup;
            // Launch a threadlet that increments T then halts, with higher priority.
            let (_t, hart_id) = ThreadOptions::threadlet_new_auto_with_arg(
                ostd::arch::riscv::threadlet::threadlet_test_entry, 2, 3, true,
            );
            ostd::arch::riscv::threadlet::threadlet_yield();
            let val = ostd::arch::riscv::threadlet::threadlet_test_get_t();
            let val_s = ostd::arch::riscv::threadlet::threadlet_test_get_s();
            ostd::early_println!("[init] threadlet test done: T={} S={}", val, val_s);

            threadlet_wakeup(hart_id);
            ostd::arch::riscv::threadlet::threadlet_yield();


            let new_val = ostd::arch::riscv::threadlet::threadlet_test_get_t();
            let val_s2 = ostd::arch::riscv::threadlet::threadlet_test_get_s();
            ostd::early_println!("[init] threadlet test done: T={} S={}", new_val, val_s2);
        }
    }
}
