use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::Ordering;

use aether_frame::libs::spin::{LocalIrqDisabled, SpinLock};
use aether_frame::process::{RunReason, RunResult};
use aether_process::{BuildError, BuiltProcess};
use aether_vfs::{OpenFileDescription, PollEvents, SharedWaitListener, WaitListener};

use super::{
    ChildEvent, ChildEventKind, DispatchWork, FileWaitRegistration, KernelProcess,
    PendingPollRegistration, Pid, ProcessBlock, ProcessBox, ProcessIdentity, ProcessManager,
    ProcessServices, ProcessState, ProcessStateSnapshot, ScheduleEvent, ZombieProcess,
};
use crate::arch::ArchContext;
use crate::arch::{
    PageFaultAccessType, UserExceptionClass, UserExceptionDetails, classify_user_exception,
    exception_signal,
};
use crate::credentials::Credentials;
use crate::fs::{FdTable, FileDescriptor};
use crate::rootfs::ProcessFsContext;
use crate::signal::{SignalDelivery, SignalFdFile, SignalInfo, SignalState};
use crate::syscall::{self, BlockResult, BlockType, SyscallArgs, SyscallDisposition};

struct FileBlockListener {
    pid: Pid,
    queue: Arc<SpinLock<alloc::collections::VecDeque<Pid>, LocalIrqDisabled>>,
    enqueued: Arc<SpinLock<alloc::collections::BTreeSet<Pid>, LocalIrqDisabled>>,
    pending: Arc<core::sync::atomic::AtomicBool>,
}

impl WaitListener for FileBlockListener {
    fn wake(&self, _events: PollEvents) {
        let should_queue = {
            let mut enqueued = self.enqueued.lock();
            enqueued.insert(self.pid)
        };
        if should_queue {
            self.queue.lock().push_back(self.pid);
        }
        self.pending.store(true, Ordering::Release);
        aether_frame::preempt::request_reschedule();
    }
}

impl ProcessManager {
    fn enqueue_runnable_pid(&mut self, pid: Pid, assigned_cpu: usize) {
        crate::processor::enqueue_runnable_pid(pid, assigned_cpu);
    }

    fn unregister_file_wait_registrations(&mut self, pid: Pid) {
        let Some(registrations) = self.file_wait_registrations.remove(&pid) else {
            return;
        };
        for registration in registrations {
            let file = registration.file.lock();
            let _ = file.unregister_waiter(registration.waiter_id);
        }
    }

    fn blocked_file_ready_result(&self, process: &KernelProcess) -> Option<BlockResult> {
        match process.state {
            ProcessState::Blocked(ProcessBlock::File { fd, events }) => {
                let descriptor = process.files.get(fd)?;
                let ready = descriptor
                    .file
                    .lock()
                    .poll(events | PollEvents::ALWAYS)
                    .ok()?;
                ready
                    .intersects(events | PollEvents::ALWAYS)
                    .then_some(BlockResult::File { ready: true })
            }
            ProcessState::Blocked(ProcessBlock::Poll { .. }) => {
                if process.pending_file_waits.iter().any(|registration| {
                    let Some(descriptor) = process.files.get(registration.fd) else {
                        return true;
                    };
                    descriptor
                        .file
                        .lock()
                        .poll(registration.events | PollEvents::ALWAYS)
                        .map(|ready| ready.intersects(registration.events | PollEvents::ALWAYS))
                        .unwrap_or(false)
                }) {
                    Some(BlockResult::Poll { timed_out: false })
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    fn publish_timer_deadline(&self) {
        crate::runtime::publish_next_timer_deadline(self.next_timer_deadline_nanos);
    }

    fn refresh_next_timer_deadline(&mut self) {
        self.next_timer_deadline_nanos = self
            .blocked_timers
            .first_key_value()
            .map(|(&deadline, _)| deadline);
        self.publish_timer_deadline();
    }

    fn note_timer_deadline_inserted(&mut self, deadline_nanos: u64) {
        self.next_timer_deadline_nanos = Some(
            self.next_timer_deadline_nanos
                .map_or(deadline_nanos, |current| current.min(deadline_nanos)),
        );
        self.publish_timer_deadline();
    }

    fn note_timer_deadline_removed(&mut self, deadline_nanos: u64) {
        if self.next_timer_deadline_nanos == Some(deadline_nanos) {
            self.refresh_next_timer_deadline();
        }
    }

    pub(crate) fn timer_deadline_due(&self, current_nanos: u64) -> bool {
        self.next_timer_deadline_nanos
            .is_some_and(|deadline| deadline <= current_nanos)
    }

    pub(crate) fn next_timer_deadline_nanos(&self) -> Option<u64> {
        self.next_timer_deadline_nanos
    }

    pub(crate) fn clone_fd_table_for_fork(
        parent: &FdTable,
        child_signals: &SignalState,
    ) -> FdTable {
        let mut table = FdTable::empty();
        for (fd, descriptor) in parent.entries() {
            let (flags, child_signalfd) = {
                let file = descriptor.file.lock();
                let flags = file.flags();
                let child_signalfd = file
                    .node()
                    .file()
                    .and_then(|ops| ops.as_any().downcast_ref::<SignalFdFile>())
                    .map(|signalfd| signalfd.with_signal_state(child_signals.clone()));
                (flags, child_signalfd)
            };
            let cloned = if let Some(signalfd) = child_signalfd {
                let node = aether_vfs::FileNode::new("signalfd", signalfd);
                FileDescriptor {
                    file: Arc::new(SpinLock::new(OpenFileDescription::new(node, flags))),
                    filesystem: descriptor.filesystem,
                    location: descriptor.location.clone(),
                    cloexec: descriptor.cloexec,
                }
            } else {
                descriptor.clone()
            };
            table.insert_at(*fd, cloned);
        }
        table
    }

    fn track_child_link(&mut self, parent: Option<Pid>, child: Pid) {
        let Some(parent) = parent else {
            return;
        };
        self.parent_children
            .entry(parent)
            .or_default()
            .insert(child);
    }

    fn untrack_child_link(&mut self, parent: Option<Pid>, child: Pid) {
        let Some(parent) = parent else {
            return;
        };
        let Some(children) = self.parent_children.get_mut(&parent) else {
            return;
        };
        children.remove(&child);
        if children.is_empty() {
            self.parent_children.remove(&parent);
        }
    }

    fn ensure_file_wait_registration(&mut self, pid: Pid, process: &KernelProcess) {
        if self.file_wait_registrations.contains_key(&pid) {
            return;
        }
        let registrations: Vec<PendingPollRegistration> = match process.state {
            ProcessState::Blocked(ProcessBlock::File { fd, events }) => {
                vec![PendingPollRegistration { fd, events }]
            }
            ProcessState::Blocked(ProcessBlock::Poll { .. }) => process.pending_file_waits.clone(),
            _ => return,
        };
        let listener: SharedWaitListener = Arc::new(FileBlockListener {
            pid,
            queue: self.file_wait_queue.clone(),
            enqueued: self.file_wait_enqueued.clone(),
            pending: self.file_wait_pending.clone(),
        });
        let mut waiter_registrations = Vec::new();
        for registration in registrations {
            let Some(descriptor) = process.files.get(registration.fd) else {
                continue;
            };
            let file = descriptor.file.clone();
            let wait_events = registration.events | PollEvents::ALWAYS;
            let waiter_id = {
                let file_guard = file.lock();
                file_guard
                    .register_waiter(wait_events, listener.clone())
                    .ok()
                    .flatten()
            };
            if let Some(waiter_id) = waiter_id {
                waiter_registrations.push(FileWaitRegistration { file, waiter_id });
            }
        }
        if !waiter_registrations.is_empty() {
            self.file_wait_registrations
                .insert(pid, waiter_registrations);
        }
    }

    fn block_from_disposition(&mut self, pid: Pid, process: &mut KernelProcess, block: BlockType) {
        process.wake_result = None;
        process.state = ProcessState::Blocked(match block {
            BlockType::Timer {
                target_nanos,
                request_nanos,
                rmtp,
                flags,
            } => ProcessBlock::Timer {
                target_nanos,
                request_nanos,
                rmtp,
                flags,
            },
            BlockType::File { fd, events } => ProcessBlock::File { fd, events },
            BlockType::Poll { deadline_nanos, .. } => ProcessBlock::Poll { deadline_nanos },
            BlockType::Futex { uaddr, bitset } => ProcessBlock::Futex { uaddr, bitset },
            BlockType::SignalSuspend => ProcessBlock::SignalSuspend,
            BlockType::Vfork { child } => ProcessBlock::Vfork { child },
            BlockType::WaitChild {
                pid,
                status_ptr,
                options,
            } => ProcessBlock::WaitChild {
                pid,
                status_ptr,
                options,
            },
        });
        if matches!(
            process.state,
            ProcessState::Blocked(ProcessBlock::File { .. } | ProcessBlock::Poll { .. })
        ) {
            self.ensure_file_wait_registration(pid, process);
        }
    }

    fn wake_process_with_result(&mut self, pid: Pid, result: BlockResult) {
        let Some(state) = self.processes.get(&pid).map(|process| process.state) else {
            return;
        };
        if !matches!(state, ProcessState::Blocked(_)) {
            return;
        }

        self.untrack_blocked_process(pid, state);
        let Some(process) = self.processes.get_mut(&pid) else {
            return;
        };
        process.wake_result = Some(result);
        process.state = ProcessState::Runnable;
        let assigned_cpu = process.assigned_cpu;
        let _ = process;
        self.enqueue_runnable_pid(pid, assigned_cpu);
    }

    fn track_blocked_process(&mut self, pid: Pid, state: ProcessState) {
        let ProcessState::Blocked(block) = state else {
            return;
        };

        match block {
            ProcessBlock::File { .. } => {
                self.blocked_files.insert(pid);
            }
            ProcessBlock::Poll { deadline_nanos } => {
                self.blocked_files.insert(pid);
                if let Some(deadline_nanos) = deadline_nanos {
                    self.blocked_timers
                        .entry(deadline_nanos)
                        .or_default()
                        .insert(pid);
                    self.note_timer_deadline_inserted(deadline_nanos);
                }
            }
            ProcessBlock::Timer { target_nanos, .. } => {
                self.blocked_timers
                    .entry(target_nanos)
                    .or_default()
                    .insert(pid);
                self.note_timer_deadline_inserted(target_nanos);
            }
            ProcessBlock::Futex { uaddr, .. } => {
                self.blocked_futexes.entry(uaddr).or_default().insert(pid);
            }
            ProcessBlock::SignalSuspend
            | ProcessBlock::Vfork { .. }
            | ProcessBlock::WaitChild { .. } => {}
        }
    }

    fn untrack_blocked_process(&mut self, pid: Pid, state: ProcessState) {
        let ProcessState::Blocked(block) = state else {
            return;
        };

        match block {
            ProcessBlock::File { .. } => {
                self.blocked_files.remove(&pid);
                self.unregister_file_wait_registrations(pid);
            }
            ProcessBlock::Poll { deadline_nanos } => {
                self.blocked_files.remove(&pid);
                self.unregister_file_wait_registrations(pid);
                if let Some(deadline_nanos) = deadline_nanos
                    && let Some(waiters) = self.blocked_timers.get_mut(&deadline_nanos)
                {
                    waiters.remove(&pid);
                    if waiters.is_empty() {
                        self.blocked_timers.remove(&deadline_nanos);
                        self.note_timer_deadline_removed(deadline_nanos);
                    }
                }
            }
            ProcessBlock::Timer { target_nanos, .. } => {
                if let Some(waiters) = self.blocked_timers.get_mut(&target_nanos) {
                    waiters.remove(&pid);
                    if waiters.is_empty() {
                        self.blocked_timers.remove(&target_nanos);
                        self.note_timer_deadline_removed(target_nanos);
                    }
                }
            }
            ProcessBlock::Futex { uaddr, .. } => {
                if let Some(waiters) = self.blocked_futexes.get_mut(&uaddr) {
                    waiters.remove(&pid);
                    if waiters.is_empty() {
                        self.blocked_futexes.remove(&uaddr);
                    }
                }
            }
            ProcessBlock::SignalSuspend
            | ProcessBlock::Vfork { .. }
            | ProcessBlock::WaitChild { .. } => {}
        }
    }

    pub fn new() -> Self {
        Self {
            next_pid: 1,
            processes: alloc::collections::BTreeMap::new(),
            zombies: alloc::collections::BTreeMap::new(),
            parent_children: alloc::collections::BTreeMap::new(),
            child_events: alloc::collections::BTreeMap::new(),
            blocked_files: alloc::collections::BTreeSet::new(),
            blocked_timers: alloc::collections::BTreeMap::new(),
            next_timer_deadline_nanos: None,
            blocked_futexes: alloc::collections::BTreeMap::new(),
            file_wait_queue: Arc::new(SpinLock::new(alloc::collections::VecDeque::new())),
            file_wait_enqueued: Arc::new(SpinLock::new(alloc::collections::BTreeSet::new())),
            file_wait_pending: Arc::new(core::sync::atomic::AtomicBool::new(false)),
            file_wait_registrations: alloc::collections::BTreeMap::new(),
            initial_files: crate::fs::FdTable::empty(),
            initial_fs: None,
        }
    }

    pub fn set_initial_files(&mut self, files: crate::fs::FdTable) {
        self.initial_files = files;
    }

    pub fn set_initial_fs(&mut self, fs: ProcessFsContext) {
        self.initial_fs = Some(fs);
    }

    pub fn spawn_task(
        &mut self,
        _name: &str,
        parent: Option<Pid>,
        task: BuiltProcess,
    ) -> Result<Pid, BuildError> {
        let pid = self.next_pid;
        self.next_pid = self.next_pid.saturating_add(1);
        let identity = ProcessIdentity {
            pid,
            parent,
            process_group: pid,
            session: pid,
        };
        self.track_child_link(parent, pid);

        self.processes.insert(
            pid,
            Box::new(KernelProcess {
                identity,
                task,
                credentials: Credentials::root(),
                prctl: crate::process::PrctlState::for_exec_path(_name),
                assigned_cpu: aether_frame::arch::cpu::current_cpu_index(),
                kernel_context: None,
                kernel_cpu: None,
                pending_exec: None,
                pending_syscall: None,
                pending_syscall_name: "",
                pending_file_waits: Vec::new(),
                mmap_regions: Vec::new(),
                vfork_parent: None,
                clear_child_tid: None,
                files: self.initial_files.clone(),
                fs: self.initial_fs.clone().ok_or(BuildError::EmptyProgram)?,
                umask: 0o022,
                signals: SignalState::new(),
                wake_result: None,
                state: ProcessState::Runnable,
            }),
        );
        self.enqueue_runnable_pid(pid, aether_frame::arch::cpu::current_cpu_index());
        Ok(pid)
    }

    pub(crate) fn allocate_pid(&mut self) -> Pid {
        let pid = self.next_pid;
        self.next_pid = self.next_pid.saturating_add(1);
        pid
    }

    #[allow(dead_code)]
    pub fn dispatch_next<S: ProcessServices>(&mut self, services: S) -> ScheduleEvent {
        match self.take_next_process() {
            DispatchWork::Idle => ScheduleEvent::Idle,
            DispatchWork::Event(event) => event,
            DispatchWork::Process(mut process) => {
                let result = process.task.process.run();
                self.finish_process(process, result, services)
            }
            DispatchWork::KernelSyscall(process) => self.resume_pending_syscall(process, services),
        }
    }

    pub fn take_next_process(&mut self) -> DispatchWork {
        crate::processor::try_take_current_cpu_work(self).unwrap_or(DispatchWork::Idle)
    }

    pub(crate) fn take_next_process_for_pid(
        &mut self,
        pid: Pid,
        current_cpu: usize,
    ) -> Option<DispatchWork> {
        let mut process = self.processes.remove(&pid)?;
        if process.assigned_cpu != current_cpu {
            let assigned_cpu = process.assigned_cpu;
            self.processes.insert(pid, process);
            self.enqueue_runnable_pid(pid, assigned_cpu);
            return None;
        }

        if process.pending_syscall.is_none() {
            match process
                .signals
                .take_next_delivery(crate::arch::supports_user_handlers())
            {
                SignalDelivery::None => {}
                SignalDelivery::Ignored(_) => {}
                SignalDelivery::Deliver(info, action) => {
                    if crate::arch::supports_user_handlers() {
                        match crate::arch::deliver_signal_to_user(&mut process, info, action) {
                            Ok(()) => {}
                            Err(_) => {
                                if info.signal != crate::signal::SIGCHLD {
                                    process.state = ProcessState::Exited(128 + info.signal as i32);
                                }
                            }
                        }
                    } else {
                        process.state = ProcessState::Exited(128 + info.signal as i32);
                    }
                }
                SignalDelivery::Exit(info) => {
                    process.state = ProcessState::Exited(128 + info.signal as i32);
                }
                SignalDelivery::Stop(info) => {
                    process.state = ProcessState::Stopped(info.signal);
                    self.notify_child_event(
                        process.identity.parent,
                        ChildEvent {
                            pid,
                            kind: ChildEventKind::Stopped(info.signal),
                        },
                    );
                    return Some(DispatchWork::Event(ScheduleEvent::Interrupted {
                        pid,
                        vector: 0,
                    }));
                }
                SignalDelivery::Continue(_) => {
                    if matches!(process.state, ProcessState::Stopped(_)) {
                        process.state = ProcessState::Runnable;
                        self.notify_child_event(
                            process.identity.parent,
                            ChildEvent {
                                pid,
                                kind: ChildEventKind::Continued,
                            },
                        );
                        self.processes.insert(pid, process);
                        self.enqueue_runnable_pid(pid, current_cpu);
                        return Some(DispatchWork::Event(ScheduleEvent::Interrupted {
                            pid,
                            vector: 0,
                        }));
                    }
                }
            }
        }

        if let ProcessState::Exited(status) = process.state {
            self.finish_process_exit(&mut process);
            let parent = process.identity.parent;
            self.zombies.insert(pid, ZombieProcess { parent });
            self.notify_child_event(
                parent,
                ChildEvent {
                    pid,
                    kind: ChildEventKind::Exited(status),
                },
            );
            return Some(DispatchWork::Event(ScheduleEvent::Exited { pid, status }));
        }

        if process.pending_syscall.is_some() {
            process.state = ProcessState::Running;
            return Some(DispatchWork::KernelSyscall(process));
        }

        process.state = ProcessState::Running;
        Some(DispatchWork::Process(process))
    }

    pub fn finish_process<S: ProcessServices>(
        &mut self,
        mut process: ProcessBox,
        result: RunResult,
        services: S,
    ) -> ScheduleEvent {
        let pid = process.identity.pid;
        let event = match result.reason {
            RunReason::Syscall => {
                let syscall_number = result.context.syscall_number();
                let syscall_args = SyscallArgs::from_context(&result.context);
                process.pending_syscall = Some(super::PendingSyscall {
                    number: syscall_number,
                    args: syscall_args,
                });
                let dispatch = Self::dispatch_pending_syscall(&mut process, services);
                return self.finish_syscall_dispatch(process, syscall_number, dispatch);
            }
            RunReason::Interrupt { vector } => {
                process.state = ProcessState::Runnable;
                ScheduleEvent::Interrupted { pid, vector }
            }
            RunReason::Exception {
                vector,
                error_code,
                fault_address,
            } => {
                let details = UserExceptionDetails {
                    vector,
                    error_code,
                    fault_address,
                    instruction_pointer: process.task.process.context().instruction_pointer(),
                };

                match classify_user_exception(details) {
                    UserExceptionClass::PageFault(page_fault)
                        if process
                            .task
                            .address_space
                            .handle_page_fault(page_fault.address, page_fault.error_code)
                            .unwrap_or(false) =>
                    {
                        process.state = ProcessState::Runnable;
                        ScheduleEvent::Interrupted { pid, vector }
                    }
                    UserExceptionClass::PageFault(page_fault) => {
                        let access = match page_fault.access {
                            PageFaultAccessType::Read => "read",
                            PageFaultAccessType::Write => "write",
                            PageFaultAccessType::Execute => "execute",
                        };
                        log::error!(
                            "user page fault: pid={} rip={:#x} addr={:#x} access={} present={} user={} ifetch={} error_code={:#x}",
                            pid,
                            page_fault.instruction_pointer,
                            page_fault.address,
                            access,
                            page_fault.present,
                            page_fault.from_user,
                            page_fault.instruction_fetch,
                            page_fault.error_code,
                        );
                        process.signals.enqueue(SignalInfo::kernel(
                            crate::signal::SIGSEGV,
                            error_code as i32,
                        ));
                        process.state = ProcessState::Runnable;
                        ScheduleEvent::Interrupted { pid, vector }
                    }
                    UserExceptionClass::Signal { signal, details } => {
                        log::error!(
                            "user exception: pid={} vector={} rip={:#x} addr={:#x} error_code={:#x}",
                            pid,
                            details.vector,
                            details.instruction_pointer,
                            details.fault_address,
                            details.error_code,
                        );
                        process
                            .signals
                            .enqueue(SignalInfo::kernel(signal, error_code as i32));
                        process.state = ProcessState::Runnable;
                        ScheduleEvent::Interrupted { pid, vector }
                    }
                    UserExceptionClass::Fatal(details) => {
                        if let Some(signal) = exception_signal(details.vector) {
                            process
                                .signals
                                .enqueue(SignalInfo::kernel(signal, error_code as i32));
                            process.state = ProcessState::Runnable;
                            ScheduleEvent::Interrupted { pid, vector }
                        } else {
                            process.state = ProcessState::Faulted { vector, error_code };
                            ScheduleEvent::Faulted {
                                pid,
                                vector,
                                error_code,
                            }
                        }
                    }
                }
            }
        };
        self.requeue_process(process);
        event
    }

    pub(crate) fn dispatch_pending_syscall_direct<S: ProcessServices>(
        process: &mut KernelProcess,
        services: S,
    ) -> syscall::SyscallDispatch {
        let pending = process
            .pending_syscall
            .expect("dispatch_pending_syscall requires a saved syscall");
        let mut context = super::context::ProcessSyscallContext { process, services };
        let dispatch = syscall::dispatch(pending.number, &mut context, pending.args);
        context.process.pending_syscall_name = dispatch.name;
        dispatch
    }

    pub(crate) fn dispatch_pending_syscall<S: ProcessServices>(
        process: &mut KernelProcess,
        services: S,
    ) -> syscall::SyscallDispatch {
        let kernel_stack_top = process.task.process.kernel_stack_top();
        aether_frame::process::run_on_kernel_stack(kernel_stack_top, || {
            Self::dispatch_pending_syscall_direct(process, services)
        })
    }

    #[allow(dead_code)]
    pub fn resume_pending_syscall<S: ProcessServices>(
        &mut self,
        mut process: ProcessBox,
        services: S,
    ) -> ScheduleEvent {
        let pending = process
            .pending_syscall
            .expect("resume_pending_syscall requires a saved syscall");
        let dispatch = Self::dispatch_pending_syscall(&mut process, services);
        self.finish_syscall_dispatch(process, pending.number, dispatch)
    }

    pub(crate) fn finish_syscall_dispatch(
        &mut self,
        mut process: ProcessBox,
        number: u64,
        dispatch: syscall::SyscallDispatch,
    ) -> ScheduleEvent {
        let pid = process.identity.pid;
        let event = match dispatch.disposition {
            SyscallDisposition::Return(value) => {
                if matches!(process.state, ProcessState::Running) {
                    let replaced_image = self.commit_pending_exec(&mut process);
                    process.pending_syscall = None;
                    process.wake_result = None;
                    if !replaced_image {
                        process
                            .task
                            .process
                            .context_mut()
                            .set_return_value(match value {
                                Ok(value) => value,
                                Err(error) => error.errno() as u64,
                            });
                    }
                    process.state = ProcessState::Runnable;
                }
                ScheduleEvent::Syscall {
                    pid,
                    number,
                    name: dispatch.name,
                }
            }
            SyscallDisposition::Exit(status) => {
                process.pending_syscall = None;
                process.state = ProcessState::Exited(status);
                ScheduleEvent::Exited { pid, status }
            }
            SyscallDisposition::Block(block) => {
                self.block_from_disposition(pid, &mut process, block);
                ScheduleEvent::Syscall {
                    pid,
                    number,
                    name: dispatch.name,
                }
            }
        };

        self.requeue_process(process);
        event
    }

    fn commit_pending_exec(&mut self, process: &mut KernelProcess) -> bool {
        let Some(new_task) = process.pending_exec.take() else {
            return false;
        };
        process.task = new_task;
        process.mmap_regions.clear();
        process.signals.prepare_for_exec();
        process.wake_result = None;
        true
    }

    fn requeue_process(&mut self, mut process: ProcessBox) {
        let pid = process.identity.pid;
        match process.state {
            ProcessState::Runnable => {
                self.enqueue_runnable_pid(pid, process.assigned_cpu);
                self.processes.insert(pid, process);
            }
            ProcessState::Stopped(_) | ProcessState::Blocked(_) => {
                self.ensure_file_wait_registration(pid, &process);
                if let Some(result) = self.blocked_file_ready_result(&process) {
                    self.unregister_file_wait_registrations(pid);
                    process.wake_result = Some(result);
                    process.state = ProcessState::Runnable;
                    self.enqueue_runnable_pid(pid, process.assigned_cpu);
                    self.processes.insert(pid, process);
                } else {
                    self.track_blocked_process(pid, process.state);
                    self.processes.insert(pid, process);
                }
            }
            ProcessState::Exited(_) => {
                let status = match process.state {
                    ProcessState::Exited(status) => status,
                    _ => 0,
                };
                self.finish_process_exit(&mut process);
                if let Some(parent) = process.vfork_parent.take() {
                    self.wake_vfork_parent(parent, pid);
                }
                let parent = process.identity.parent;
                self.zombies.insert(pid, ZombieProcess { parent });
                self.notify_child_event(
                    parent,
                    ChildEvent {
                        pid,
                        kind: ChildEventKind::Exited(status),
                    },
                );
            }
            ProcessState::Running | ProcessState::Faulted { .. } => {}
        }
    }

    #[allow(dead_code)]
    pub fn has_live_processes(&self) -> bool {
        !self.processes.is_empty() || crate::processor::has_running_processes()
    }

    #[allow(dead_code)]
    pub fn state_snapshots(&self) -> Vec<ProcessStateSnapshot> {
        let mut snapshots = self
            .processes
            .iter()
            .map(|(pid, process)| ProcessStateSnapshot {
                pid: *pid,
                state: process.state,
            })
            .collect::<Vec<_>>();
        snapshots.extend(crate::processor::running_state_snapshots());
        snapshots
    }

    pub fn procfs_snapshot(&self, pid: Pid) -> Option<crate::process::ProcFsProcessSnapshot> {
        if let Some(process) = self.processes.get(&pid) {
            return Some(crate::process::ProcFsProcessSnapshot {
                pid,
                parent: process.identity.parent,
                state: process.state,
                name: *process.prctl.name_bytes(),
                credentials: process.credentials.clone(),
                umask: process.umask,
            });
        }

        crate::processor::running_procfs_snapshot(pid)
    }

    pub(crate) fn insert_cloned_process(&mut self, process: KernelProcess) -> Pid {
        let pid = process.identity.pid;
        let assigned_cpu = process.assigned_cpu;
        self.track_child_link(process.identity.parent, pid);
        self.processes.insert(pid, Box::new(process));
        self.enqueue_runnable_pid(pid, assigned_cpu);
        pid
    }

    fn finish_process_exit(&mut self, process: &mut KernelProcess) {
        let Some(clear_child_tid) = process.clear_child_tid.take() else {
            return;
        };

        let zero = 0u32.to_ne_bytes();
        if let Ok(written) = process.task.address_space.write(clear_child_tid, &zero)
            && written == zero.len()
        {
            let _ = self.wake_futex(clear_child_tid, u32::MAX, 1);
        }
    }

    pub(crate) fn reap_child_event(
        &mut self,
        parent_pid: Pid,
        requested: i32,
        options: u64,
    ) -> Option<ChildEvent> {
        let event = {
            let events = self.child_events.get_mut(&parent_pid)?;
            let index = events.iter().position(|event| {
                (requested == -1 || event.pid == requested as u32)
                    && super::util::child_event_matches_options(event.kind, options)
            })?;
            let event = events.remove(index)?;
            if events.is_empty() {
                self.child_events.remove(&parent_pid);
            }
            event
        };
        if matches!(event.kind, ChildEventKind::Exited(_)) {
            let parent = self
                .zombies
                .remove(&event.pid)
                .and_then(|zombie| zombie.parent);
            self.untrack_child_link(parent, event.pid);
        }
        Some(event)
    }

    pub(crate) fn has_child(&self, parent_pid: Pid, requested: i32) -> bool {
        let Some(children) = self.parent_children.get(&parent_pid) else {
            return false;
        };
        requested == -1 || children.contains(&(requested as u32))
    }

    pub(crate) fn wake_ready_file_blocks(&mut self) {
        if !self.file_wait_pending.swap(false, Ordering::AcqRel) {
            return;
        }

        let mut ready = alloc::collections::BTreeSet::new();
        let mut queue = self.file_wait_queue.lock();
        let mut enqueued = self.file_wait_enqueued.lock();
        while let Some(pid) = queue.pop_front() {
            enqueued.remove(&pid);
            ready.insert(pid);
        }
        drop(enqueued);
        drop(queue);

        for pid in ready {
            match self.processes.get(&pid).map(|process| process.state) {
                Some(ProcessState::Blocked(ProcessBlock::File { .. })) => {
                    self.wake_process_with_result(pid, BlockResult::File { ready: true });
                }
                Some(ProcessState::Blocked(ProcessBlock::Poll { .. })) => {
                    self.wake_process_with_result(pid, BlockResult::Poll { timed_out: false });
                }
                _ => {}
            }
        }
    }

    pub(crate) fn wake_expired_timers(&mut self, current_nanos: u64) {
        loop {
            let Some((&deadline, _)) = self.blocked_timers.first_key_value() else {
                self.next_timer_deadline_nanos = None;
                self.publish_timer_deadline();
                break;
            };
            if deadline > current_nanos {
                self.next_timer_deadline_nanos = Some(deadline);
                self.publish_timer_deadline();
                break;
            }

            let Some(waiters) = self.blocked_timers.remove(&deadline) else {
                self.refresh_next_timer_deadline();
                break;
            };
            for pid in waiters {
                let Some(process) = self.processes.get(&pid) else {
                    continue;
                };
                match process.state {
                    ProcessState::Blocked(ProcessBlock::Timer { rmtp, flags, .. }) => {
                        self.wake_process_with_result(
                            pid,
                            BlockResult::Timer {
                                completed: true,
                                remaining_nanos: 0,
                                rmtp,
                                is_absolute: (flags & crate::syscall::abi::TIMER_ABSTIME) != 0,
                            },
                        );
                    }
                    ProcessState::Blocked(ProcessBlock::Poll { .. }) => {
                        self.wake_process_with_result(pid, BlockResult::Poll { timed_out: true });
                    }
                    _ => {}
                }
            }
        }
    }

    pub(crate) fn notify_signal(&mut self, pid: Pid, info: SignalInfo) {
        let mut child_event = None;
        let mut queue_assigned_cpu = None;
        let Some(mut_state) = self.processes.get(&pid).map(|process| process.state) else {
            return;
        };

        if info.signal == crate::signal::SIGCONT && matches!(mut_state, ProcessState::Stopped(_)) {
            let Some(process) = self.processes.get_mut(&pid) else {
                return;
            };
            process.state = ProcessState::Runnable;
            queue_assigned_cpu = Some(process.assigned_cpu);
            child_event = Some((
                process.identity.parent,
                ChildEvent {
                    pid,
                    kind: ChildEventKind::Continued,
                },
            ));
        }

        let mut unblock_block = None;
        {
            let Some(process) = self.processes.get_mut(&pid) else {
                return;
            };
            process.signals.enqueue(info);

            if matches!(
                process.state,
                ProcessState::Blocked(ProcessBlock::SignalSuspend)
            ) {
                process.signals.leave_sigsuspend();
                process.wake_result = Some(BlockResult::SignalInterrupted);
                process.state = ProcessState::Runnable;
                queue_assigned_cpu = Some(process.assigned_cpu);
            }

            if let ProcessState::Blocked(ProcessBlock::Timer {
                target_nanos,
                request_nanos,
                rmtp,
                flags,
            }) = process.state
            {
                let _ = request_nanos;
                let current_nanos = aether_frame::interrupt::timer::nanos_since_boot();
                process.wake_result = Some(if current_nanos < target_nanos {
                    BlockResult::Timer {
                        completed: false,
                        remaining_nanos: target_nanos.saturating_sub(current_nanos),
                        rmtp,
                        is_absolute: (flags & crate::syscall::abi::TIMER_ABSTIME) != 0,
                    }
                } else {
                    BlockResult::Timer {
                        completed: true,
                        remaining_nanos: 0,
                        rmtp,
                        is_absolute: (flags & crate::syscall::abi::TIMER_ABSTIME) != 0,
                    }
                });
                unblock_block = Some(process.state);
                process.state = ProcessState::Runnable;
                queue_assigned_cpu = Some(process.assigned_cpu);
            }

            if let ProcessState::Blocked(ProcessBlock::Poll { .. }) = process.state {
                process.wake_result = Some(BlockResult::SignalInterrupted);
                unblock_block = Some(process.state);
                process.state = ProcessState::Runnable;
                queue_assigned_cpu = Some(process.assigned_cpu);
            }
        }
        if let Some(assigned_cpu) = queue_assigned_cpu {
            self.enqueue_runnable_pid(pid, assigned_cpu);
        }
        if let Some(blocked_state) = unblock_block {
            self.untrack_blocked_process(pid, blocked_state);
        }

        if let Some((parent, event)) = child_event {
            self.notify_child_event(parent, event);
        }
    }

    pub(crate) fn wake_vfork_parent(&mut self, parent_pid: Pid, child_pid: Pid) {
        let Some(parent) = self.processes.get_mut(&parent_pid) else {
            return;
        };
        if matches!(
            parent.state,
            ProcessState::Blocked(ProcessBlock::Vfork { child }) if child == child_pid
        ) {
            parent.wake_result = Some(BlockResult::CompletedValue {
                value: child_pid as u64,
            });
            parent.state = ProcessState::Runnable;
            let assigned_cpu = parent.assigned_cpu;
            let _ = parent;
            self.enqueue_runnable_pid(parent_pid, assigned_cpu);
        }
    }

    pub(crate) fn send_signal(&mut self, pid: Pid, info: SignalInfo) -> bool {
        if self.processes.contains_key(&pid) {
            self.notify_signal(pid, info);
            true
        } else {
            false
        }
    }

    pub(crate) fn send_signal_process_group(
        &mut self,
        process_group: Pid,
        info: SignalInfo,
    ) -> usize {
        let targets = self
            .processes
            .iter()
            .filter_map(|(&pid, process)| {
                (process.identity.process_group == process_group).then_some(pid)
            })
            .collect::<Vec<_>>();
        for pid in &targets {
            self.notify_signal(*pid, info);
        }
        targets.len()
    }

    pub(crate) fn wake_futex(&mut self, uaddr: u64, bitset: u32, count: usize) -> usize {
        let mut woke = 0usize;
        let Some(waiters) = self.blocked_futexes.get(&uaddr).cloned() else {
            return 0;
        };
        for pid in waiters {
            if woke >= count {
                break;
            }
            let Some(state) = self.processes.get(&pid).map(|process| process.state) else {
                continue;
            };
            let ProcessState::Blocked(ProcessBlock::Futex {
                uaddr: wait_uaddr,
                bitset: wait_bitset,
            }) = state
            else {
                continue;
            };
            if wait_uaddr != uaddr || (wait_bitset & bitset) == 0 {
                continue;
            }
            self.untrack_blocked_process(pid, state);
            let Some(process) = self.processes.get_mut(&pid) else {
                continue;
            };
            process.wake_result = Some(BlockResult::Futex { woke: true });
            process.state = ProcessState::Runnable;
            let assigned_cpu = process.assigned_cpu;
            let _ = process;
            self.enqueue_runnable_pid(pid, assigned_cpu);
            woke += 1;
        }
        woke
    }

    pub(crate) fn requeue_futex(
        &mut self,
        from: u64,
        to: u64,
        wake_count: usize,
        requeue_count: usize,
        bitset: u32,
    ) -> usize {
        let woke = self.wake_futex(from, bitset, wake_count);
        let mut moved = 0usize;
        let Some(waiters) = self.blocked_futexes.get(&from).cloned() else {
            return woke;
        };
        for pid in waiters {
            if moved >= requeue_count {
                break;
            }
            let Some(state) = self.processes.get(&pid).map(|process| process.state) else {
                continue;
            };
            let ProcessState::Blocked(ProcessBlock::Futex {
                uaddr,
                bitset: wait_bitset,
            }) = state
            else {
                continue;
            };
            if uaddr != from || (wait_bitset & bitset) == 0 {
                continue;
            }
            self.untrack_blocked_process(pid, state);
            let new_state = {
                let Some(process) = self.processes.get_mut(&pid) else {
                    continue;
                };
                process.state = ProcessState::Blocked(ProcessBlock::Futex {
                    uaddr: to,
                    bitset: wait_bitset,
                });
                process.state
            };
            self.track_blocked_process(pid, new_state);
            moved += 1;
        }
        woke
    }

    fn notify_child_event(&mut self, parent: Option<Pid>, event: ChildEvent) {
        let Some(parent_pid) = parent else {
            return;
        };

        let mut woke_waiter = false;
        if let Some(parent_process) = self.processes.get_mut(&parent_pid)
            && let ProcessState::Blocked(ProcessBlock::WaitChild {
                pid: waited_pid,
                status_ptr,
                options,
            }) = parent_process.state
            && (waited_pid == -1 || waited_pid == event.pid as i32)
            && super::util::child_event_matches_options(event.kind, options)
        {
            let nohang = (options & 1) != 0;
            if !nohang {
                if status_ptr != 0 {
                    let raw = super::util::wait_status(event.kind).to_ne_bytes();
                    let _ = parent_process.task.address_space.write(status_ptr, &raw);
                }
                parent_process.wake_result = Some(BlockResult::CompletedValue {
                    value: event.pid as u64,
                });
                parent_process.state = ProcessState::Runnable;
                let assigned_cpu = parent_process.assigned_cpu;
                let _ = parent_process;
                self.enqueue_runnable_pid(parent_pid, assigned_cpu);
                if matches!(event.kind, ChildEventKind::Exited(_)) {
                    let parent = self
                        .zombies
                        .remove(&event.pid)
                        .and_then(|zombie| zombie.parent);
                    self.untrack_child_link(parent, event.pid);
                }
                woke_waiter = true;
            }
        }

        if !woke_waiter {
            self.child_events
                .entry(parent_pid)
                .or_default()
                .push_back(event);
        }

        match event.kind {
            ChildEventKind::Exited(_status) => {}
            ChildEventKind::Stopped(signal) => {
                self.notify_signal(parent_pid, SignalInfo::child_stop(signal))
            }
            ChildEventKind::Continued => {
                self.notify_signal(parent_pid, SignalInfo::child_continue())
            }
        }
    }
}

impl Default for ProcessManager {
    fn default() -> Self {
        Self::new()
    }
}
