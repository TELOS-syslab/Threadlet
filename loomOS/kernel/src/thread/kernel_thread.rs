// SPDX-License-Identifier: MPL-2.0

use ostd::{
    cpu::{CpuSet, PinCurrentCpu},
    task::{Task, TaskOptions},
};

use super::{oops, AsThread, Thread};
use crate::{prelude::*, sched::priority::Priority};

/// The inner data of a kernel thread.
struct KernelThread;

/// Entry tracked for a hardware-managed threadlet.
struct ThreadletTaskEntry {
    cpu_id: usize,
    hart_id: u32,
    task: Arc<Task>,
}

/// Global registry for hardware-managed threadlet tasks.
static THREADLET_TASKS: SpinLock<Vec<ThreadletTaskEntry>> = SpinLock::new(Vec::new());

const THREADLET_CTX_REGS: usize = 32;
const THREADLET_CTX_REG_BYTES: usize = 8;

fn threadlet_ctx_linear_vaddr_from_paddr(paddr: usize) -> usize {
    const LINEAR_MAPPING_BASE_VADDR: usize = 0xffff_8000_0000_0000;
    paddr
        .checked_add(LINEAR_MAPPING_BASE_VADDR)
        .expect("threadlet ctx linear vaddr overflow")
}

#[allow(unsafe_code)]
fn write_threadlet_initial_ctx_mem(
    ctx_base_paddr: usize,
    hart_id: u32,
    sp: usize,
    gp: usize,
    tp: usize,
    arg: u64,
) {
    let thread_offset = (hart_id as usize)
        .checked_mul(THREADLET_CTX_REGS * THREADLET_CTX_REG_BYTES)
        .expect("threadlet ctx offset overflow");
    let base_paddr = ctx_base_paddr
        .checked_add(thread_offset)
        .expect("threadlet ctx paddr overflow");
    let base_vaddr = threadlet_ctx_linear_vaddr_from_paddr(base_paddr);

    unsafe {
        let regs = base_vaddr as *mut u64;
        for reg in 0..THREADLET_CTX_REGS {
            core::ptr::write_volatile(regs.add(reg), 0);
        }
        core::ptr::write_volatile(regs.add(2), sp as u64);
        core::ptr::write_volatile(regs.add(3), gp as u64);
        core::ptr::write_volatile(regs.add(4), tp as u64);
        core::ptr::write_volatile(regs.add(10), arg);
        core::arch::asm!("fence rw, rw", options(nostack));
    }
}

fn current_cpu_id() -> usize {
    let guard = ostd::task::disable_preempt();
    guard.current_cpu().as_usize()
}

/// Destroys a hardware-managed threadlet and releases its associated task and stack.
/// The threadlet should have executed `threadlet_halt`
pub fn threadlet_end(cpu_id: usize, hart_id: u32) {
    let mut guard = THREADLET_TASKS.lock();
    if let Some(pos) = guard
        .iter()
        .position(|e| e.cpu_id == cpu_id && e.hart_id == hart_id)
    {
        let _entry = guard.swap_remove(pos);
    } else {
        ostd::early_println!(
            "[threadlet] threadlet_end: no tracked task entry for cpu {} hart {}",
            cpu_id,
            hart_id
        );
    }
}


/// Options to create or spawn a new kernel thread.
pub struct ThreadOptions {
    func: Option<Box<dyn Fn() + Send + Sync>>,
    priority: Priority,
    cpu_affinity: CpuSet,
}

impl ThreadOptions {
    /// Creates the thread options with the thread function.
    pub fn new<F>(func: F) -> Self
    where
        F: Fn() + Send + Sync + 'static,
    {
        let cpu_affinity = CpuSet::new_full();
        Self {
            func: Some(Box::new(func)),
            priority: Priority::default(),
            cpu_affinity,
        }
    }

    /// Sets the priority of the new thread.
    pub fn priority(mut self, priority: Priority) -> Self {
        self.priority = priority;
        self
    }

    /// Sets the CPU affinity of the new thread.
    pub fn cpu_affinity(mut self, cpu_affinity: CpuSet) -> Self {
        self.cpu_affinity = cpu_affinity;
        self
    }
}

impl ThreadOptions {
    /// Builds a new kernel thread without running it immediately.
    pub fn build(mut self) -> Arc<Task> {
        let task_fn = self.func.take().unwrap();
        let thread_fn = move || {
            let _ = oops::catch_panics_as_oops(task_fn);
            // Ensure that the thread exits.
            current_thread!().exit();
        };

        Arc::new_cyclic(|weak_task| {
            let thread = {
                let kernel_thread = KernelThread;
                let priority = self.priority;
                let cpu_affinity = self.cpu_affinity;
                Arc::new(Thread::new(
                    weak_task.clone(),
                    kernel_thread,
                    priority,
                    cpu_affinity,
                ))
            };

            TaskOptions::new(thread_fn).data(thread).build().unwrap()
        })
    }

    /// Builds a new kernel thread and runs it immediately.
    #[track_caller]
    pub fn spawn(self) -> Arc<Thread> {
        let task = self.build();
        let thread = task.as_thread().unwrap().clone();
        thread.run();
        thread
    }

    /// Creates and initializes a hardware-managed threadlet.
    pub fn threadlet_new(_hart_id_hint: u32, entry: fn()) -> Arc<Thread> {
        // Reuse the existing builder path to allocate task structures and compute SP/TP.
        let task = ThreadOptions::new(entry).build();
        let thread = task.as_thread().unwrap().clone();
        // Ensure this thread is not scheduled by OS.
        thread.mark_hardware_managed();

        let sp = task.stack_pointer();
        let tp = task.tls_pointer();
        // Read current gp as the kernel global pointer value.
        let gp = ostd::arch::riscv::threadlet::read_current_gp();

        // Program the hardware threadlet via THREAD_CREATE (auto-assign a free hart).
        let ret =
            ostd::arch::riscv::threadlet::threadlet_create(entry as *const () as usize);
        if ret < 0 {
            println!("[kernel:: threadlet_new]: no free hart");
            return thread;
        }
        else {
            println!("[kernel:: threadlet_new] hartid {} is assigned", ret);
        }
        let hart_id = ret as u32;
        let cpu_id = current_cpu_id();

        {
            let mut guard = THREADLET_TASKS.lock();
            guard.push(ThreadletTaskEntry {
                cpu_id,
                hart_id,
                task: task.clone(),
            });
        }

        ostd::arch::riscv::threadlet::threadlet_set_sp(hart_id, sp);
        ostd::arch::riscv::threadlet::threadlet_set_gp(hart_id, gp);
        ostd::arch::riscv::threadlet::threadlet_set_tp(hart_id, tp);

        let entry_pc = entry as usize;
        println!(
            "[kernel] threadlet_new: hart {} entry_pc {:#x} sp {:#x} gp {:#x} tp {:#x}",
            hart_id, entry_pc, sp, gp, tp
        );

        thread
    }

    /// This variant uses a 64-bit argument defaulted to 0.
    pub fn threadlet_new_auto(entry: fn(), priority: u32) -> (Arc<Thread>, u32) {
        Self::threadlet_new_auto_with_arg(entry, priority, 0, true)
    }

    /// Creates and initializes a hardware-managed threadlet on an auto-selected hart,
    /// then sets its scheduling priority and a 64-bit argument (in a0).
    pub fn threadlet_new_auto_with_arg(
        entry: fn(),
        priority: u32,
        arg: u64,
        auto_wakeup: bool,
    ) -> (Arc<Thread>, u32) {
        Self::threadlet_new_auto_with_arg_common(entry, priority, arg, auto_wakeup, None)
    }

    /// Creates a hardware-managed threadlet and mirrors its initial GPR context
    /// into the hardware context backing store before it can be woken.
    pub fn threadlet_new_auto_with_arg_with_mem(
        entry: fn(),
        priority: u32,
        arg: u64,
        auto_wakeup: bool,
        ctx_base_paddr: usize,
    ) -> (Arc<Thread>, u32) {
        Self::threadlet_new_auto_with_arg_common(
            entry,
            priority,
            arg,
            auto_wakeup,
            Some(ctx_base_paddr),
        )
    }

    fn threadlet_new_auto_with_arg_common(
        entry: fn(),
        priority: u32,
        arg: u64,
        auto_wakeup: bool,
        ctx_base_paddr: Option<usize>,
    ) -> (Arc<Thread>, u32) {
        let task = ThreadOptions::new(|| {}).build();
        let thread = task.as_thread().unwrap().clone();
        thread.mark_hardware_managed();

        let sp = task.stack_pointer();
        let tp = task.tls_pointer();
        let entry_pc = entry as usize;

        let hart_id = ostd::arch::riscv::threadlet::threadlet_create(entry as *const () as usize);
        if hart_id < 0 {
            println!("[kernel] threadlet_new_auto: no free hart");
            return (thread, u32::MAX);
        }
        let hart_id = hart_id as u32;
        let cpu_id = current_cpu_id();
        {
            let mut guard = THREADLET_TASKS.lock();
            guard.push(ThreadletTaskEntry {
                cpu_id,
                hart_id,
                task: task.clone(),
            });
        }
        let gp = ostd::arch::riscv::threadlet::read_current_gp();
        ostd::arch::riscv::threadlet::threadlet_set_sp(hart_id, sp);
        ostd::arch::riscv::threadlet::threadlet_set_gp(hart_id, gp);
        ostd::arch::riscv::threadlet::threadlet_set_tp(hart_id, tp);
        ostd::arch::riscv::threadlet::threadlet_set_a0(hart_id, arg);
        if let Some(ctx_base_paddr) = ctx_base_paddr {
            write_threadlet_initial_ctx_mem(ctx_base_paddr, hart_id, sp, gp, tp, arg);
        }
        // Enable by setting its priority after context is initialized.
        ostd::arch::riscv::threadlet::threadlet_set_priority(hart_id, priority);
        // println!(
        //     "[kernel] threadlet_new_auto_with_arg: hart {} entry_pc {:#x} sp {:#x} gp {:#x} tp {:#x} arg {:#x} prio {} (wakeup)",
        //     hart_id, entry_pc, sp, gp, tp, arg, priority
        // );
        if auto_wakeup {
            ostd::arch::riscv::threadlet::threadlet_syn_print(5, 50);
            ostd::arch::riscv::threadlet::threadlet_syn_print(5, 50);
            ostd::arch::riscv::threadlet::threadlet_wakeup(hart_id);
        }
        (thread, hart_id)
    }
}
