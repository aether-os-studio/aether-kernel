use alloc::collections::{BTreeSet, VecDeque};
use alloc::vec::Vec;

use aether_frame::boot;
use aether_frame::libs::percpu::{PerCpu, PerCpuError};
use aether_frame::libs::spin::{LocalIrqDisabled, SpinLock};
use aether_frame::process::KernelContext;

use crate::process::{
    DispatchWork, Pid, ProcFsProcessSnapshot, ProcessManager, ProcessState, ProcessStateSnapshot,
    RunningProcessSnapshot,
};

struct ProcessorState {
    scheduler_context: KernelContext,
    current_pid: Option<Pid>,
    current_process: Option<RunningProcessSnapshot>,
    run_queue: SpinLock<LocalRunQueue, LocalIrqDisabled>,
}

struct LocalRunQueue {
    runnable: VecDeque<Pid>,
    queued: BTreeSet<Pid>,
}

impl LocalRunQueue {
    fn new() -> Self {
        Self {
            runnable: VecDeque::new(),
            queued: BTreeSet::new(),
        }
    }

    fn enqueue(&mut self, pid: Pid) -> bool {
        if !self.queued.insert(pid) {
            return false;
        }
        self.runnable.push_back(pid);
        true
    }

    fn dequeue(&mut self) -> Option<Pid> {
        let pid = self.runnable.pop_front()?;
        self.queued.remove(&pid);
        Some(pid)
    }
}

impl ProcessorState {
    fn new() -> Self {
        Self {
            scheduler_context: KernelContext::default(),
            current_pid: None,
            current_process: None,
            run_queue: SpinLock::new(LocalRunQueue::new()),
        }
    }
}

static PROCESSORS: PerCpu<ProcessorState, { boot::MAX_CPUS }> = PerCpu::uninit();

fn current_cpu() -> usize {
    aether_frame::arch::cpu::current_cpu_index()
}

pub(crate) fn init_current_cpu() -> Result<(), &'static str> {
    let cpu_index = current_cpu();
    match PROCESSORS.init(cpu_index, ProcessorState::new()) {
        Ok(()) | Err(PerCpuError::AlreadyInitialized) => {}
        Err(PerCpuError::InvalidCpu) => return Err("invalid cpu index for processor state"),
        Err(PerCpuError::Uninitialized) => return Err("unexpected processor state init failure"),
    }

    PROCESSORS
        .with_mut(cpu_index, |processor| {
            processor.current_pid = None;
            processor.current_process = None;
            aether_frame::process::install_scheduler_context(&mut processor.scheduler_context);
        })
        .map_err(|_| "failed to install cpu-local scheduler context")
}

pub(crate) fn current_pid() -> Option<Pid> {
    PROCESSORS
        .with(current_cpu(), |processor| processor.current_pid)
        .ok()
        .flatten()
}

fn set_current_execution(snapshot: Option<RunningProcessSnapshot>) {
    let _ = PROCESSORS.with_mut(current_cpu(), |processor| {
        processor.current_pid = snapshot.as_ref().map(|process| process.pid);
        processor.current_process = snapshot;
    });
}

pub(crate) fn with_current_process<R>(
    snapshot: RunningProcessSnapshot,
    f: impl FnOnce() -> R,
) -> R {
    struct ResetCurrentProcess;

    impl Drop for ResetCurrentProcess {
        fn drop(&mut self) {
            crate::processor::set_current_execution(None);
        }
    }

    set_current_execution(Some(snapshot));
    let reset = ResetCurrentProcess;
    let result = f();
    core::mem::drop(reset);
    result
}

pub(crate) fn enqueue_runnable_pid(pid: Pid, cpu_index: usize) {
    let target_cpu = if PROCESSORS.get(cpu_index).is_ok() {
        cpu_index
    } else {
        current_cpu()
    };

    let queued = PROCESSORS
        .with(target_cpu, |processor| {
            processor.run_queue.lock().enqueue(pid)
        })
        .unwrap_or(false);
    if queued {
        aether_frame::preempt::request_reschedule_cpu(target_cpu);
    }
}

pub(crate) fn dequeue_next_runnable_pid(cpu_index: usize) -> Option<Pid> {
    PROCESSORS
        .with(cpu_index, |processor| processor.run_queue.lock().dequeue())
        .ok()
        .flatten()
}

pub(crate) fn try_take_current_cpu_work(processes: &mut ProcessManager) -> Option<DispatchWork> {
    let cpu_index = current_cpu();
    loop {
        let pid = dequeue_next_runnable_pid(cpu_index)?;
        if let Some(work) = processes.take_next_process_for_pid(pid, cpu_index) {
            return Some(work);
        }
    }
}

pub(crate) fn has_running_processes() -> bool {
    (0..boot::MAX_CPUS).any(|cpu_index| {
        PROCESSORS
            .with(cpu_index, |processor| processor.current_process.is_some())
            .unwrap_or(false)
    })
}

pub(crate) fn running_state_snapshots() -> Vec<ProcessStateSnapshot> {
    let mut snapshots = Vec::new();
    for cpu_index in 0..boot::MAX_CPUS {
        let Some(snapshot) = PROCESSORS
            .with(cpu_index, |processor| {
                processor
                    .current_process
                    .as_ref()
                    .map(|process| process.pid)
            })
            .ok()
            .flatten()
        else {
            continue;
        };
        snapshots.push(ProcessStateSnapshot {
            pid: snapshot,
            state: ProcessState::Running,
        });
    }
    snapshots
}

pub(crate) fn running_procfs_snapshot(pid: Pid) -> Option<ProcFsProcessSnapshot> {
    for cpu_index in 0..boot::MAX_CPUS {
        let Some(snapshot) = PROCESSORS
            .with(cpu_index, |processor| processor.current_process.clone())
            .ok()
            .flatten()
        else {
            continue;
        };
        if snapshot.pid == pid {
            return Some(ProcFsProcessSnapshot {
                pid: snapshot.pid,
                parent: snapshot.parent,
                state: ProcessState::Running,
                name: snapshot.name,
                credentials: snapshot.credentials,
                umask: snapshot.umask,
            });
        }
    }
    None
}
