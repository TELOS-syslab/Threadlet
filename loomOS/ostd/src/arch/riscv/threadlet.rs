//! RISC-V threadlet instructions.
#![allow(unsafe_code)]

// Bring in the exported macro `cpu_local_cell!` so we can declare per-CPU cells here.
use crate::cpu_local_cell;
use riscv::register::{sstatus, satp};

// No per-hart terminate instruction in the public doc.
#[inline(always)]
pub fn threadlet_create(entry_pc: usize) -> i32 {
    unsafe {
        let ret: i32;
        // .insn r opcode(0x0B), funct3(0), funct7(0x1), rd=ret(hartid), rs1=pc, rs2=x0
        core::arch::asm!(
            ".insn r 0x0B, 0, 0x1, {ret}, {pc}, x0",
            ret = out(reg) ret,
            pc = in(reg) entry_pc,
        );
        ret
    }
}

/// Set the base pc for the interrupt threadlet#1 (THREAD_SET_BASE). Each time interrupt occurs,
/// the core will switch to threadlet#1 and jump to base pc.
#[inline(always)]
pub fn threadlet_set_base(base_addr: usize) {
    unsafe {
        core::arch::asm!(
            ".insn r 0x0B, 0, 0xf, x0, {base}, x0",
            base = in(reg) base_addr,
        );
    }
}

/// Set the physical base address for hardware threadlet context save.
#[inline(always)]
pub fn threadlet_set_ctx_base(base_paddr: usize) {
    unsafe {
        core::arch::asm!(
            ".insn r 0x0B, 0, 0x19, x0, {base}, x0",
            base = in(reg) base_paddr,
            options(nostack)
        );
    }
}


// No create-on-hart instruction in the public doc.

/// Initialize and enable threadlet on current core (THREAD_INIT).
#[inline(always)]
pub fn threadlet_init() {
    unsafe {
        // THREAD_INIT: funct7=0x0, rd=x0, rs1=x0, rs2=x0
        core::arch::asm!(
            ".insn r 0x0B, 0, 0x0, x0, x0, x0",
            options(nostack, nomem)
        );
        crate::early_println!("[threadlet] THREAD_INIT done");
    }
}

/// Enable interrupt handling for threadlets on current core (THREAD_INTR_ENABLE).
#[inline(always)]
pub fn enable_interrupt_threadlet() {
    unsafe {
        core::arch::asm!(
            ".insn r 0x0B, 0, 0x3, x0, x0, x0",
            options(nostack, nomem)
        );
        crate::early_println!("[threadlet] enable interrupt_threadlet done");
    }
}

/// Enable HW debug print using THREAD_PRINT_ENABLE.
#[inline(always)]
pub fn threadlet_print_enable() {
    unsafe {
        core::arch::asm!(
            ".insn r 0x0B, 0, 0xc, x0, x0, x0",
            options(nostack, nomem)
        );
        crate::early_println!("[threadlet] enable HW debug done");
    }
}
/// Disable HW debug print using THREAD_PRINT_DISABLE.
#[inline(always)]
pub fn threadlet_print_disable() {
    unsafe {
        core::arch::asm!(
            ".insn r 0x0B, 0, 0xd, x0, x0, x0",
            options(nostack, nomem)
        );
        crate::early_println!("[threadlet] disable HW debug done done");
    }
}

/// Enable DCache debug print using THREAD_DCACHE_PRINT_EN.
#[inline(always)]
pub fn threadlet_dcache_print_enable() {
    unsafe {
        core::arch::asm!(
            ".insn r 0x0B, 0, 0x10, x0, x0, x0",
            options(nostack, nomem)
        );
    }
}

/// Disable DCache debug print using THREAD_DCACHE_PRINT_DIS.
#[inline(always)]
pub fn threadlet_dcache_print_disable() {
    unsafe {
        core::arch::asm!(
            ".insn r 0x0B, 0, 0x11, x0, x0, x0",
            options(nostack, nomem)
        );
    }
}

/// Program a DCache monitor slot for the current threadlet.
///
/// - `addr`: address to monitor (only the cacheline matters).
/// - `slot`: slot index (low bits are used).
#[inline(always)]
pub fn threadlet_dcache_monitor_set(addr: usize, slot: usize) {
    unsafe {
        core::arch::asm!(
            ".insn r 0x0B, 0, 0x12, x0, {addr}, {slot}",
            addr = in(reg) addr,
            slot = in(reg) slot,
            options(nostack, nomem)
        );
    }
}

/// Clear a DCache monitor slot for the current threadlet.
#[inline(always)]
pub fn threadlet_dcache_monitor_clear(addr: usize, slot: usize) {
    unsafe {
        core::arch::asm!(
            ".insn r 0x0B, 0, 0x13, x0, {addr}, {slot}",
            addr = in(reg) addr,
            slot = in(reg) slot,
            options(nostack, nomem)
        );
    }
}

/// Monitor `addr` and then yield the current threadlet until it is woken by a DCache probe match.
#[inline(always)]
pub fn threadlet_dcache_mwait(addr: usize, slot: usize) {
    unsafe {
        core::arch::asm!(
            ".insn r 0x0B, 0, 0x14, x0, {addr}, {slot}",
            addr = in(reg) addr,
            slot = in(reg) slot,
            options(nostack, nomem)
        );
    }
}


/// Halt the current running threadlet (THREAD_HALT). No return value.
#[inline(always)]
pub fn threadlet_halt() -> ! {
    unsafe {
        core::arch::asm!(
            ".insn r 0x0B, 0, 0x2, x0, x0, x0",
            options(noreturn)
        );
    }
}


/// Yield the current running threadlet (THREAD_YIELD).
#[inline(always)]
pub fn threadlet_yield() {
    unsafe {
        core::arch::asm!(
            ".insn r 0x0B, 0, 0x8, x0, x0, x0",
            options(nostack, nomem)
        );
    }
}

/// Pass the current timeslice and trigger one immediate scheduling decision (THREAD_PASS).
/// This does not mark current threadlet as unrunnable.
#[inline(always)]
pub fn threadlet_pass() {
    unsafe {
        core::arch::asm!(
            ".insn r 0x0B, 0, 0x17, x0, x0, x0",
            options(nostack, nomem)
        );
    }
}

/// Register an event-triggered wakeup target for the current threadlet.
/// The function is aborted.
/// 
/// 
/// 
/// 
/// 

/// The net ERET for the current running threadlet is regarded as sret+threadlet_yield. 
/// Only can be used for interrupt threadlet (THREAD_ERET_HINT).
#[inline(always)]
pub fn threadlet_eret_hint(){
    unsafe {
        core::arch::asm!(
            ".insn r 0x0b, 0x1, 0x00, x0, x0, x0",
            options(nostack, nomem)
        );
    }
}

/// Returns the id of the currently running threadlet on this core.
#[inline(always)]
pub fn threadlet_current() -> i32 {
    let id: i32;
    unsafe {
        core::arch::asm!(
            ".insn r 0x0B, 0, 0x7, {id}, x0, x0",
            id = out(reg) id,
            options(nomem, nostack)
        );
    }
    id
}

/// Reads the current a0 (x10) register value.
#[inline(always)]
pub fn threadlet_get_a0() -> u64 {
    let val: u64;
    unsafe {
        core::arch::asm!("mv {0}, a0", out(reg) val, options(nomem, nostack));
    }
    val
}

/// Set SP for the target hart's threadlet.
#[inline(always)]
pub fn threadlet_set_sp(hart_id: u32, sp: usize) {
    unsafe {
        // Use THREAD_CTX_WRITE (funct7=4). Fix rd to x2 to select `sp`.
        core::arch::asm!(
            ".insn r 0x0B, 0, 0x4, x2, {hart}, {val}",
            hart = in(reg) hart_id as usize,
            val = in(reg) sp,
            options(nostack)
        );
        // crate::early_println!(
        //     "[threadlet] THREAD_CTX_WRITE sp: hart={} value={:#x}",
        //     hart_id, sp
        // );
    }
}

/// Sets the argument register a0 (x10) for the target hart's threadlet.
#[inline(always)]
pub fn threadlet_set_a0(hart_id: u32, arg: u64) {
    unsafe {
        core::arch::asm!(
            ".insn r 0x0B, 0, 0x4, x10, {hart}, {val}",
            hart = in(reg) hart_id as usize,
            val = in(reg) arg,
            options(nostack)
        );
        // crate::early_println!(
        //     "[threadlet] THREAD_CTX_WRITE a0: hart={} value={:#x}",
        //     hart_id, arg
        // );
    }
}

/// expensive.
#[inline(always)]
pub fn threadlet_set_a0_quiet(hart_id: u32, arg: u64) {
    unsafe {
        core::arch::asm!(
            ".insn r 0x0B, 0, 0x4, x10, {hart}, {val}",
            hart = in(reg) hart_id as usize,
            val = in(reg) arg,
            options(nostack)
        );
    }
}

/// Set GP for the target hart's threadlet.
#[inline(always)]
pub fn threadlet_set_gp(hart_id: u32, gp: usize) {
    unsafe {
        // Use THREAD_CTX_WRITE (funct7=4). Fix rd to x3 to select `gp`.
        core::arch::asm!(
            ".insn r 0x0B, 0, 0x4, x3, {hart}, {val}",
            hart = in(reg) hart_id as usize,
            val = in(reg) gp,
            options(nostack)
        );
        // crate::early_println!(
        //     "[threadlet] THREAD_CTX_WRITE gp: hart={} value={:#x}",
        //     hart_id, gp
        // );
    }
}

/// Set TP for the target hart's threadlet.
#[inline(always)]
pub fn threadlet_set_tp(hart_id: u32, tp: usize) {
    unsafe {
        // Use THREAD_CTX_WRITE (funct7=4). Fix rd to x4 to select `tp`.
        core::arch::asm!(
            ".insn r 0x0B, 0, 0x4, x4, {hart}, {val}",
            hart = in(reg) hart_id as usize,
            val = in(reg) tp,
            options(nostack)
        );
        // crate::early_println!(
        //     "[threadlet] THREAD_CTX_WRITE tp: hart={} value={:#x}",
        //     hart_id, tp
        // );
    }
}

/// read current GP for debug
#[inline(always)]
pub fn read_current_gp() -> usize {
    let gp_val: usize;
    unsafe {
        core::arch::asm!("mv {0}, gp", out(reg) gp_val, options(nomem, nostack));
    }
    gp_val
}

/// read current TP for debug
#[inline(always)]
pub fn read_current_tp() -> usize {
    let tp_val: usize;
    unsafe {
        core::arch::asm!("mv {0}, tp", out(reg) tp_val, options(nomem, nostack));
    }
    tp_val
}

/// init on BSP and APs. Set private regs for all threadlets.// 
// 
#[inline(always)]
pub fn threadlet_init_on_core(){
    // Issue THREAD_INIT on this CPU to enable threadlet hardware.
    threadlet_init();
}

/// Hardware-level debug print using THREAD_SYN_PRINT.
#[inline(always)]
pub fn threadlet_syn_print(stage: u64, data: u64) {
    unsafe {
        core::arch::asm!(
            ".insn r 0x0B, 0, 0xe, x0, {stage}, {data}",
            stage = in(reg) stage,
            data = in(reg) data,
            options(nostack, nomem)
        );
    }
}

/// Atomically switch interrupt handling to the interrupt threadlet on this core.
#[inline(always)]
pub fn enable_interrupt_threadlet_with_base(val: usize) {
    unsafe {
        riscv::interrupt::disable();
        threadlet_set_base(val);
        crate::early_println!(
            "[threadlet] enable_interrupt_threadlet_with_base: base=0x{:x}",
            val
        );

        threadlet_syn_print(5,0);
        threadlet_syn_print(5,0);
        threadlet_syn_print(5,0);
        enable_interrupt_threadlet();
        threadlet_syn_print(5,1);
        threadlet_syn_print(5,1);
        threadlet_syn_print(5,1);

        // threadlet_print_enable();
        riscv::interrupt::enable();
    }
}


pub fn threadlet_todo()
{

// check . vmfence  page
// 
}

/* ----------------------- Per-CPU hart id pool (lock-free) ----------------------- */

// use the exported `cpu_local_cell!` macro directly

const MAX_HARTS_PER_CORE: usize = 16;

struct HartPool {
    buf: [u32; MAX_HARTS_PER_CORE],
    head: usize,
    tail: usize,
    len: usize,
}

impl HartPool {
    const fn new() -> Self {
        Self { buf: [0; MAX_HARTS_PER_CORE], head: 0, tail: 0, len: 0 }
    }

    fn clear(&mut self) {
        self.head = 0;
        self.tail = 0;
        self.len = 0;
    }

    fn push(&mut self, id: u32) -> bool {
        if self.len >= MAX_HARTS_PER_CORE { return false; }
        self.buf[self.tail] = id;
        self.tail = (self.tail + 1) % MAX_HARTS_PER_CORE;
        self.len += 1;
        true
    }

    fn pop(&mut self) -> Option<u32> {
        if self.len == 0 { return None; }
        let id = self.buf[self.head];
        self.head = (self.head + 1) % MAX_HARTS_PER_CORE;
        self.len -= 1;
        Some(id)
    }
}

cpu_local_cell! {
    static HART_POOL: HartPool = HartPool::new();
}

#[inline(always)]
pub fn hart_pool_init(ids: &[u32]) {
    let pool = HART_POOL.as_mut_ptr();

    let pool = unsafe { &mut *pool };
    pool.clear();
    for &id in ids.iter().take(MAX_HARTS_PER_CORE) {
        let _ = pool.push(id);
    }
}

#[inline(always)]
pub fn hart_alloc() -> Option<u32> {
    let pool = HART_POOL.as_mut_ptr();

    unsafe { (*pool).pop() }
}

#[inline(always)]
pub fn hart_free(id: u32) -> bool {
    let pool = HART_POOL.as_mut_ptr();
    unsafe { (*pool).push(id) }
}

// read current SP for debug
#[inline(always)]
pub fn read_current_sp() -> usize {
    let sp_val: usize;
    unsafe { core::arch::asm!("mv {0}, sp", out(reg) sp_val, options(nomem, nostack)); }
    sp_val
}

// Deprecated: ---- Per-threadlet CSR programming (THREAD_CSR_SET) ----
// Do not use these intructions, instead use normal riscv instructions to set CSRs.
// Encoding (THREAD_CSR_SET): funct7=0xB, rd=csr_index, rs1=threadlet_id, rs2=value
const CSR_IDX_SSTATUS: usize = 0;
const CSR_IDX_STVEC: usize = 4;
const CSR_IDX_SATP: usize = 11;
const CSR_IDX_MSTATUS: usize = 12;
const CSR_IDX_MEDELEG: usize = 14;
const CSR_IDX_MIDELEG: usize = 15;
const CSR_IDX_SSCRATCH: usize = 6;
const CSR_IDX_MTVEC: usize = 17;

/// Set per-threadlet sstatus CSR for the given threadlet id.
#[inline(always)]
pub fn threadlet_set_sstatus(threadlet_id: u32, sstatus_val: usize) {
    unsafe {
        core::arch::asm!(
            ".insn r 0x0B, 0, 0xB, x0, {id}, {val}",
            id = in(reg) threadlet_id as usize,
            val = in(reg) sstatus_val,
            options(nostack)
        );
        crate::early_println!(
            "[threadlet] THREAD_CSR_SET sstatus: hart={} value={:#x}",
            threadlet_id, sstatus_val
        );
    }
}

/// Set per-threadlet stvec CSR for the given threadlet id.
#[inline(always)]
pub fn threadlet_set_stvec(threadlet_id: u32, stvec_val: usize) {
    unsafe {
        core::arch::asm!(
            ".insn r 0x0B, 0, 0xB, x4, {id}, {val}",
            id = in(reg) threadlet_id as usize,
            val = in(reg) stvec_val,
            options(nostack)
        );
        crate::early_println!(
            "[threadlet] THREAD_CSR_SET stvec: hart={} value={:#x}",
            threadlet_id, stvec_val
        );
    }
}

/// Set per-threadlet satp CSR for the given threadlet id.
#[inline(always)]
pub fn threadlet_set_satp(threadlet_id: u32, satp_val: usize) {
    unsafe {
        core::arch::asm!(
            ".insn r 0x0B, 0, 0xB, x11, {id}, {val}",
            id = in(reg) threadlet_id as usize,
            val = in(reg) satp_val,
            options(nostack)
        );
        crate::early_println!(
            "[threadlet] THREAD_CSR_SET satp: hart={} value={:#x}",
            threadlet_id, satp_val
        );
    }
}

/// Set per-threadlet mstatus CSR for the given threadlet id.
#[inline(always)]
pub fn threadlet_set_mstatus(threadlet_id: u32, mstatus_val: usize) {
    unsafe {
        core::arch::asm!(
            ".insn r 0x0B, 0, 0xB, x12, {id}, {val}",
            id = in(reg) threadlet_id as usize,
            val = in(reg) mstatus_val,
            options(nostack)
        );
        crate::early_println!(
            "[threadlet] THREAD_CSR_SET mstatus: hart={} value={:#x}",
            threadlet_id, mstatus_val
        );
    }
}

/// Set per-threadlet mtvec CSR for the given threadlet id.
#[inline(always)]
pub fn threadlet_set_mtvec(threadlet_id: u32, mtvec_val: usize) {
    unsafe {
        core::arch::asm!(
            ".insn r 0x0B, 0, 0xB, x17, {id}, {val}",
            id = in(reg) threadlet_id as usize,
            val = in(reg) mtvec_val,
            options(nostack)
        );
        crate::early_println!(
            "[threadlet] THREAD_CSR_SET mtvec: hart={} value={:#x}",
            threadlet_id, mtvec_val
        );
    }
}

/// Set per-threadlet mideleg CSR (delegate interrupts to S-mode).
#[inline(always)]
pub fn threadlet_set_mideleg(threadlet_id: u32, mideleg_val: usize) {
    unsafe {
        core::arch::asm!(
            ".insn r 0x0B, 0, 0xB, x15, {id}, {val}",
            id = in(reg) threadlet_id as usize,
            val = in(reg) mideleg_val,
            options(nostack)
        );
        crate::early_println!(
            "[threadlet] THREAD_CSR_SET mideleg: hart={} value={:#x}",
            threadlet_id, mideleg_val
        );
    }
}

/// Set per-threadlet sscratch CSR (kernel: must be 0 to indicate S-mode origin).
#[inline(always)]
pub fn threadlet_set_sscratch(threadlet_id: u32, sscratch_val: usize) {
    unsafe {
        core::arch::asm!(
            ".insn r 0x0B, 0, 0xB, x6, {id}, {val}",
            id = in(reg) threadlet_id as usize,
            val = in(reg) sscratch_val,
            options(nostack)
        );
        crate::early_println!(
            "[threadlet] THREAD_CSR_SET sscratch: hart={} value={:#x}",
            threadlet_id, sscratch_val
        );
    }
}

/// Read raw sstatus bits on the current hart.
#[inline(always)]
fn read_sstatus_bits() -> usize {
    let val: usize;
    unsafe {
        core::arch::asm!("csrr {0}, sstatus", out(reg) val, options(nomem, nostack));
    }
    val
}

/// Read raw stvec bits on the current hart.
#[inline(always)]
fn read_stvec_bits() -> usize {
    let val: usize;
    unsafe {
        core::arch::asm!("csrr {0}, stvec", out(reg) val, options(nomem, nostack));
    }
    val
}

/// Read raw satp bits on the current hart.
#[inline(always)]
fn read_satp_bits() -> usize {
    let val: usize;
    unsafe {
        core::arch::asm!("csrr {0}, satp", out(reg) val, options(nomem, nostack));
    }
    val
}

/// Deprecated：Copy key CSRs from the current hart to the target threadlet.
#[inline(always)]
pub fn threadlet_copy_csrs_from_current(threadlet_id: u32) {
    let cur_sstatus = read_sstatus_bits();
    let cur_stvec = read_stvec_bits();
    let cur_satp = read_satp_bits();

    threadlet_set_sstatus(threadlet_id, cur_sstatus);
    threadlet_set_stvec(threadlet_id, cur_stvec);
    threadlet_set_satp(threadlet_id, cur_satp);

    threadlet_set_mstatus(threadlet_id, cur_sstatus);

    // Ensure interrupts trap to S-mode on this threadlet hart.
    const SSIP: usize = 1 << 1;
    const STIP: usize = 1 << 5;
    const SEIP: usize = 1 << 9;
    let mideleg = SSIP | STIP | SEIP;
    threadlet_set_mideleg(threadlet_id, mideleg);
    threadlet_set_sscratch(threadlet_id, 0);
}

/// Set priority for a given threadlet id on this core.
///
/// Encoding (THREAD_SET_PROI): funct7=0x5, rd=x0, rs1=threadlet_id, rs2=priority
#[inline(always)]
pub fn threadlet_set_priority(threadlet_id: u32, priority: u32) {
    unsafe {
        core::arch::asm!(
            ".insn r 0x0B, 0, 0x5, x0, {id}, {prio}",
            id = in(reg) threadlet_id as usize,
            prio = in(reg) priority as usize,
            options(nostack)
        );
    }
}


/// Hardware enforces the slice with a 32-cycle granularity.
/// Encoding (THREAD_SET_SLICE): funct7=0x15, rd=x0, rs1=threadlet_id, rs2=cycles
#[inline(always)]
pub fn threadlet_set_timeslice(threadlet_id: u32, cycles: u64) {
    unsafe {
        core::arch::asm!(
            ".insn r 0x0B, 0, 0x15, x0, {id}, {cycles}",
            id = in(reg) threadlet_id as usize,
            cycles = in(reg) cycles as usize,
            options(nostack)
        );
    }
}

/// Set deadline period for a given threadlet id on this core. (0: disable)
/// The deadline is specified in cycles, and hardware enforces it with a 32-cycle granularity

#[inline(always)]
pub fn threadlet_set_deadline(threadlet_id: u32, cycles: u64) {
    unsafe {
        core::arch::asm!(
            ".insn r 0x0B, 0, 0x16, x0, {id}, {cycles}",
            id = in(reg) threadlet_id as usize,
            cycles = in(reg) cycles as usize,
            options(nostack)
        );
    }
}

/// Wake up a given threadlet id on this core so that it becomes runnable.
///
/// Encoding (THREAD_WAKEUP): funct7=0xA, rd=x0, rs1=threadlet_id, rs2=x0
#[inline(always)]
pub fn threadlet_wakeup(threadlet_id: u32) {
    unsafe {
        core::arch::asm!(
            ".insn r 0x0B, 0, 0xA, x0, {id}, x0",
            id = in(reg) threadlet_id as usize,
            options(nostack)
        );
    }
}

// ---------------- Threadlet test helpers for kernel ----------------
static mut THREADLET_TEST_T: usize = 0;
static mut THREADLET_TEST_S: usize = 0;

/// Entry point for a simple test threadlet.
///
/// The 64-bit argument `arg` is passed in a0 according to the RISC-V
/// calling convention. We assume the threadlet never returns.
pub fn threadlet_test_entry(){
    let arg = threadlet_get_a0() as usize;

    loop {
        unsafe {
            THREADLET_TEST_T = THREADLET_TEST_T + arg;
        }
        crate::early_println!("[threadlet_test] test threadlet entry executed 1");
        unsafe {
            THREADLET_TEST_S = THREADLET_TEST_S + 3;
        }
        crate::early_println!("[threadlet_test] test threadlet entry executed 2");

        threadlet_wakeup(0);
        // Yield after one round of logic; on wakeup we resume from here
        threadlet_yield();
    }
}

/// Reads the current value of the global test variable.
#[inline(always)]
pub fn threadlet_test_get_t() -> usize {
    unsafe { THREADLET_TEST_T }
}
#[inline(always)]
pub fn threadlet_test_get_s() -> usize {
    unsafe { THREADLET_TEST_S }
}
