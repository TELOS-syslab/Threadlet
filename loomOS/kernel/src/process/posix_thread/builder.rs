// SPDX-License-Identifier: MPL-2.0

#![allow(dead_code)]

use ostd::{cpu::{CpuId, CpuSet}, task::Task, user::UserSpace};

use super::{thread_table, PosixThread, ThreadLocal};
use crate::{
    fs::{file_table::FileTable, thread_info::ThreadFsInfo},
    prelude::*,
    process::{
        posix_thread::name::ThreadName,
        signal::{sig_mask::AtomicSigMask, sig_queues::SigQueues},
        Credentials, Process,
    },
    sched::priority::Priority,
    thread::{task, Thread, Tid},
    time::{clocks::ProfClock, TimerManager},
};

/// The builder to build a posix thread
pub struct PosixThreadBuilder {
    // The essential part
    tid: Tid,
    user_space: Arc<UserSpace>,
    process: Weak<Process>,
    credentials: Credentials,

    // Optional part
    thread_name: Option<ThreadName>,
    set_child_tid: Vaddr,
    clear_child_tid: Vaddr,
    file_table: Option<Arc<SpinLock<FileTable>>>,
    fs: Option<Arc<ThreadFsInfo>>,
    sig_mask: AtomicSigMask,
    sig_queues: SigQueues,
    priority: Priority,
}

impl PosixThreadBuilder {
    pub fn new(tid: Tid, user_space: Arc<UserSpace>, credentials: Credentials) -> Self {
        Self {
            tid,
            user_space,
            process: Weak::new(),
            credentials,
            thread_name: None,
            set_child_tid: 0,
            clear_child_tid: 0,
            file_table: None,
            fs: None,
            sig_mask: AtomicSigMask::new_empty(),
            sig_queues: SigQueues::new(),
            priority: Priority::default(),
        }
    }

    pub fn process(mut self, process: Weak<Process>) -> Self {
        self.process = process;
        self
    }

    pub fn thread_name(mut self, thread_name: Option<ThreadName>) -> Self {
        self.thread_name = thread_name;
        self
    }

    pub fn set_child_tid(mut self, set_child_tid: Vaddr) -> Self {
        self.set_child_tid = set_child_tid;
        self
    }

    pub fn clear_child_tid(mut self, clear_child_tid: Vaddr) -> Self {
        self.clear_child_tid = clear_child_tid;
        self
    }

    pub fn file_table(mut self, file_table: Arc<SpinLock<FileTable>>) -> Self {
        self.file_table = Some(file_table);
        self
    }

    pub fn fs(mut self, fs: Arc<ThreadFsInfo>) -> Self {
        self.fs = Some(fs);
        self
    }

    pub fn sig_mask(mut self, sig_mask: AtomicSigMask) -> Self {
        self.sig_mask = sig_mask;
        self
    }

    pub fn priority(mut self, priority: Priority) -> Self {
        self.priority = priority;
        self
    }

    pub fn build(self) -> Arc<Task> {
        let Self {
            tid,
            user_space,
            process,
            credentials,
            thread_name,
            set_child_tid,
            clear_child_tid,
            file_table,
            fs,
            sig_mask,
            sig_queues,
            priority,
        } = self;

        let file_table =
            file_table.unwrap_or_else(|| Arc::new(SpinLock::new(FileTable::new_with_stdio())));

        let fs = fs.unwrap_or_else(|| Arc::new(ThreadFsInfo::default()));

        Arc::new_cyclic(|weak_task| {
            // Decide default CPU affinity for the new user thread.
            // On RISC-V, if this process is the init process and executable is busybox,
            // pin it to CPU0 (BSP). 
            #[cfg(target_arch = "riscv64")]
            let user_cpu_affinity = {
                let (pin_init_busybox, exe_path) = if let Some(proc_arc) = process.upgrade() {
                    let is_init = proc_arc.is_init_process();
                    let path = proc_arc.executable_path();
                    (
                        is_init
                            && (path.ends_with("/busybox")
                                || path.ends_with("bin/busybox")
                                || path.contains("busybox")),
                        path,
                    )
                } else {
                    (false, String::new())
                };
                if pin_init_busybox {
                    ostd::early_println!(
                        "[sched] RISC-V: detected busybox init ({}), pin to CPU0",
                        exe_path
                    );
                    CpuSet::from(CpuId::bsp())
                } else {
                    CpuSet::new_full()
                }
            };
            #[cfg(not(target_arch = "riscv64"))]
            let user_cpu_affinity = CpuSet::new_full();
            let posix_thread = {
                let prof_clock = ProfClock::new();
                let virtual_timer_manager = TimerManager::new(prof_clock.user_clock().clone());
                let prof_timer_manager = TimerManager::new(prof_clock.clone());

                PosixThread {
                    process,
                    tid,
                    name: Mutex::new(thread_name),
                    credentials,
                    file_table,
                    fs,
                    sig_mask,
                    sig_queues,
                    signalled_waker: SpinLock::new(None),
                    prof_clock,
                    virtual_timer_manager,
                    prof_timer_manager,
                }
            };

            // Default CPU affinity for user threads.
            // On RISC-V, pin user-space to CPU0 to avoid running any
            // potentially C-extension binaries on other CPUs.
            #[cfg(target_arch = "riscv64")]
            let cpu_affinity = user_cpu_affinity;
            let thread = Arc::new(Thread::new(
                weak_task.clone(),
                posix_thread,
                priority,
                cpu_affinity,
            ));

            let thread_local = ThreadLocal::new(set_child_tid, clear_child_tid);

            thread_table::add_thread(tid, thread.clone());
            task::create_new_user_task(user_space, thread, thread_local)
        })
    }
}
