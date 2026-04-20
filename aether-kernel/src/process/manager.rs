use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::Ordering;

use aether_frame::libs::spin::{LocalIrqDisabled, SpinLock};
use aether_frame::process::{RunReason, RunResult};
use aether_frame::time;
use aether_process::{BuildError, BuiltProcess};
use aether_vfs::{OpenFileDescription, PollEvents, SharedWaitListener, WaitListener};

use super::{
    ChildEvent, ChildEventKind, DispatchWork, FileWaitRegistration, KernelProcess,
    PendingPollRegistration, Pid, ProcessBlock, ProcessBox, ProcessIdentity, ProcessManager,
    ProcessServices, ProcessState, ProcessStateSnapshot, ScheduleEvent, WaitChildApi,
    WaitChildSelector, ZombieProcess,
};
use crate::arch::ArchContext;
use crate::arch::{
    PageFaultAccessType, UserExceptionClass, UserExceptionDetails, classify_user_exception,
    exception_signal,
};
use crate::credentials::Credentials;
use crate::errno::SysResult;
use crate::fs::{FdTable, FileDescriptor, PidFdHandle};
use crate::rootfs::ProcessFsContext;
use crate::signal::{
    SA_NOCLDSTOP, SA_NOCLDWAIT, SIG_DFL, SIG_IGN, SIGCHLD, SignalAction, SignalDelivery,
    SignalFdFile, SignalInfo, SignalState, sigbit,
};
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
    const ROBUST_LIST_HEAD_LEN: u64 = 24;
    const ROBUST_LIST_LIMIT: usize = 2048;
    const FUTEX_WAITERS: u32 = 0x8000_0000;
    const FUTEX_OWNER_DIED: u32 = 0x4000_0000;
    const FUTEX_TID_MASK: u32 = 0x3fff_ffff;

    fn read_exit_u64(process: &KernelProcess, address: u64) -> Option<u64> {
        let bytes = process
            .task
            .address_space
            .read_user_exact(address, 8)
            .ok()?;
        Some(u64::from_ne_bytes(bytes.as_slice().try_into().ok()?))
    }

    fn read_exit_i64(process: &KernelProcess, address: u64) -> Option<i64> {
        let bytes = process
            .task
            .address_space
            .read_user_exact(address, 8)
            .ok()?;
        Some(i64::from_ne_bytes(bytes.as_slice().try_into().ok()?))
    }

    fn read_exit_u32(process: &KernelProcess, address: u64) -> Option<u32> {
        let bytes = process
            .task
            .address_space
            .read_user_exact(address, 4)
            .ok()?;
        Some(u32::from_ne_bytes(bytes.as_slice().try_into().ok()?))
    }

    fn wake_futex_scopes(
        &mut self,
        process: &KernelProcess,
        uaddr: u64,
        count: usize,
        bitset: u32,
    ) {
        let private =
            crate::process::FutexKey::private(process.task.address_space.identity(), uaddr);
        let _ = self.wake_futex(private, bitset, count);
        let shared = crate::process::FutexKey::shared(uaddr);
        let _ = self.wake_futex(shared, bitset, count);
    }

    fn prepare_child_return(&mut self, process: &mut KernelProcess) {
        let Some(child_tid) = process.set_child_tid.take() else {
            return;
        };

        let raw = process.identity.pid.to_ne_bytes();
        let status = match process.task.address_space.write(child_tid, &raw) {
            Ok(written) if written == raw.len() => None,
            _ => Some(128 + crate::signal::SIGSEGV as i32),
        };

        if let Some(status) = status {
            process.state = ProcessState::Exited(status);
        }
    }

    fn queue_synchronous_fault_signal(
        &mut self,
        process: &mut KernelProcess,
        signal: u8,
        code: i32,
    ) -> bool {
        let Some(action) = process.signals.action(signal) else {
            process.state = ProcessState::Exited(128 + signal as i32);
            return false;
        };

        let blocked = process.signals.blocked();
        let has_user_handler = action.handler != SIG_DFL && action.handler != SIG_IGN;
        let deliverable = crate::arch::supports_user_handlers()
            && has_user_handler
            && (blocked & sigbit(signal)) == 0;

        if deliverable {
            process.signals.enqueue(SignalInfo::kernel(signal, code));
            process.state = ProcessState::Runnable;
            true
        } else {
            let status = 128 + signal as i32;
            if process.is_thread() {
                self.begin_thread_group_exit(
                    process.identity.thread_group,
                    process.identity.pid,
                    status,
                );
            }
            process.state = ProcessState::Exited(status);
            false
        }
    }

    fn queue_signal_for_running(&mut self, pid: Pid, info: SignalInfo) -> bool {
        let Some(cpu_index) = crate::processor::running_cpu_of(pid) else {
            return false;
        };
        self.queued_signals.entry(pid).or_default().push_back(info);
        aether_frame::preempt::request_reschedule_cpu(cpu_index);
        true
    }

    fn drain_queued_signals(&mut self, process: &mut KernelProcess) {
        let Some(signals) = self.queued_signals.remove(&process.identity.pid) else {
            return;
        };
        for info in signals {
            process.signals.enqueue(info);
        }
    }

    fn thread_group_process_group(&self, tgid: Pid) -> Option<Pid> {
        let members = self.thread_groups.get(&tgid)?;
        for pid in members {
            if let Some(process) = self.processes.get(pid) {
                return Some(process.identity.process_group);
            }
            if let Some(process_group) = crate::processor::running_process_group_of(*pid) {
                return Some(process_group);
            }
        }
        None
    }

    fn thread_group_session(&self, tgid: Pid) -> Option<Pid> {
        let members = self.thread_groups.get(&tgid)?;
        for pid in members {
            if let Some(process) = self.processes.get(pid) {
                return Some(process.identity.session);
            }
        }
        None
    }

    pub(crate) fn process_group_session(&self, process_group: Pid) -> Option<Pid> {
        self.thread_groups.keys().copied().find_map(|tgid| {
            (self.thread_group_process_group(tgid) == Some(process_group))
                .then(|| self.thread_group_session(tgid))
                .flatten()
        })
    }

    fn pick_thread_group_target(&self, tgid: Pid, signal: u8) -> Option<Pid> {
        let members = self.thread_groups.get(&tgid)?;
        let mut fallback = None;

        if members.contains(&tgid)
            && (self.processes.contains_key(&tgid)
                || crate::processor::running_cpu_of(tgid).is_some())
        {
            fallback = Some(tgid);
            if let Some(process) = self.processes.get(&tgid)
                && !crate::signal::is_blocked(process.signals.blocked(), signal)
            {
                return Some(tgid);
            }
            if crate::processor::running_cpu_of(tgid).is_some() {
                return Some(tgid);
            }
        }

        for pid in members {
            if Some(*pid) == fallback {
                continue;
            }
            if !(self.processes.contains_key(pid)
                || crate::processor::running_cpu_of(*pid).is_some())
            {
                continue;
            }

            fallback.get_or_insert(*pid);
            if let Some(process) = self.processes.get(pid)
                && !crate::signal::is_blocked(process.signals.blocked(), signal)
            {
                return Some(*pid);
            }
            if crate::processor::running_cpu_of(*pid).is_some() {
                return Some(*pid);
            }
        }

        fallback
    }

    fn cleanup_robust_word(&mut self, process: &KernelProcess, futex_uaddr: u64) {
        let Some(word) = Self::read_exit_u32(process, futex_uaddr) else {
            return;
        };
        if (word & Self::FUTEX_TID_MASK) != process.identity.pid {
            return;
        }

        let new_word = (word & Self::FUTEX_WAITERS) | Self::FUTEX_OWNER_DIED;
        let raw = new_word.to_ne_bytes();
        let Ok(written) = process.task.address_space.write(futex_uaddr, &raw) else {
            return;
        };
        if written != raw.len() {
            return;
        }

        if (word & Self::FUTEX_WAITERS) != 0 {
            self.wake_futex_scopes(process, futex_uaddr, 1, u32::MAX);
        }
    }

    fn cleanup_robust_entry(
        &mut self,
        process: &KernelProcess,
        futex_offset: i64,
        entry_addr: u64,
    ) {
        let futex_uaddr = if futex_offset < 0 {
            entry_addr.checked_sub((-futex_offset) as u64)
        } else {
            entry_addr.checked_add(futex_offset as u64)
        };
        let Some(futex_uaddr) = futex_uaddr else {
            return;
        };
        self.cleanup_robust_word(process, futex_uaddr);
    }

    fn cleanup_robust_list(&mut self, process: &KernelProcess) {
        let Some(head_addr) = process.robust_list_head else {
            return;
        };
        if process.robust_list_len < Self::ROBUST_LIST_HEAD_LEN {
            return;
        }

        let Some(list_next) = Self::read_exit_u64(process, head_addr) else {
            return;
        };
        let Some(futex_offset) = Self::read_exit_i64(process, head_addr + 8) else {
            return;
        };
        let Some(list_op_pending) = Self::read_exit_u64(process, head_addr + 16) else {
            return;
        };

        let mut entry_addr = list_next;
        for _ in 0..Self::ROBUST_LIST_LIMIT {
            if entry_addr == 0 || entry_addr == head_addr {
                break;
            }
            let Some(next) = Self::read_exit_u64(process, entry_addr) else {
                break;
            };
            self.cleanup_robust_entry(process, futex_offset, entry_addr);
            entry_addr = next;
        }

        if list_op_pending != 0 && list_op_pending != head_addr {
            self.cleanup_robust_entry(process, futex_offset, list_op_pending);
        }
    }

    fn enqueue_runnable_pid(&mut self, pid: Pid, assigned_cpu: usize) {
        crate::processor::enqueue_runnable_pid(pid, assigned_cpu);
    }

    fn parent_sigchld_action(&self, parent: Option<Pid>) -> Option<SignalAction> {
        let parent_pid = parent?;
        self.processes.get(&parent_pid)?.signals.action(SIGCHLD)
    }

    fn reap_exited_process(&mut self, process: &mut KernelProcess, status: i32) {
        let pid = process.identity.pid;
        let tgid = process.identity.thread_group;
        self.queued_signals.remove(&pid);
        process.pidfd.mark_exited();
        self.finish_process_exit(process);
        if let Some(parent) = process.vfork_parent.take() {
            self.wake_vfork_parent(parent, pid);
        }
        self.untrack_thread_group(tgid, pid);

        if process.is_thread() {
            process.pidfd.mark_reaped();
            return;
        }

        let parent = process.identity.parent;
        let sigchld_action = self.parent_sigchld_action(parent);
        let ignore_sigchld = sigchld_action
            .map(|action| action.handler == SIG_IGN)
            .unwrap_or(false);
        let no_cldwait = sigchld_action
            .map(|action| (action.flags & SA_NOCLDWAIT) != 0)
            .unwrap_or(false);

        if let Some(parent_pid) = parent
            && !ignore_sigchld
        {
            self.notify_signal(parent_pid, SignalInfo::child_exit(pid, status));
        }

        if parent.is_none() || ignore_sigchld || no_cldwait {
            process.pidfd.mark_reaped();
            self.untrack_child_link(parent, pid);
            return;
        }

        self.zombies.insert(
            pid,
            ZombieProcess {
                parent,
                process_group: process.identity.process_group,
                pidfd: process.pidfd.clone(),
            },
        );
        self.notify_child_event(
            parent,
            ChildEvent {
                pid,
                kind: ChildEventKind::Exited(status),
            },
        );
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

    pub(crate) fn arm_futex_wait(&mut self, pid: Pid, key: crate::process::FutexKey, bitset: u32) {
        self.armed_futex_waits.insert(
            pid,
            super::ArmedFutexWait {
                key,
                bitset,
                raced_wake: false,
            },
        );
    }

    pub(crate) fn disarm_futex_wait(&mut self, pid: Pid) {
        self.armed_futex_waits.remove(&pid);
    }

    fn take_armed_futex_wait(&mut self, pid: Pid) -> Option<super::ArmedFutexWait> {
        self.armed_futex_waits.remove(&pid)
    }

    fn wake_armed_futex_waits(
        &mut self,
        key: crate::process::FutexKey,
        bitset: u32,
        count: usize,
    ) -> usize {
        if count == 0 {
            return 0;
        }

        let targets = self
            .armed_futex_waits
            .iter()
            .filter_map(|(&pid, wait)| {
                (wait.key == key && (wait.bitset & bitset) != 0 && !wait.raced_wake).then_some(pid)
            })
            .take(count)
            .collect::<Vec<_>>();

        let mut woke = 0usize;
        for pid in targets {
            let Some(wait) = self.armed_futex_waits.get_mut(&pid) else {
                continue;
            };
            if wait.key != key || (wait.bitset & bitset) == 0 || wait.raced_wake {
                continue;
            }
            wait.raced_wake = true;
            woke += 1;
        }
        woke
    }

    fn requeue_armed_futex_waits(
        &mut self,
        from: crate::process::FutexKey,
        to: crate::process::FutexKey,
        requeue_count: usize,
        bitset: u32,
    ) -> usize {
        if requeue_count == 0 {
            return 0;
        }

        let targets = self
            .armed_futex_waits
            .iter()
            .filter_map(|(&pid, wait)| {
                (wait.key == from && (wait.bitset & bitset) != 0 && !wait.raced_wake).then_some(pid)
            })
            .take(requeue_count)
            .collect::<Vec<_>>();

        let mut moved = 0usize;
        for pid in targets {
            let Some(wait) = self.armed_futex_waits.get_mut(&pid) else {
                continue;
            };
            if wait.key != from || (wait.bitset & bitset) == 0 || wait.raced_wake {
                continue;
            }
            wait.key = to;
            moved += 1;
        }
        moved
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
        let entries = parent.with_entries(|parent_entries| {
            let mut child_entries = alloc::collections::BTreeMap::new();
            for (&fd, descriptor) in parent_entries {
                let (flags, child_signalfd) = {
                    let file = descriptor.file.lock();
                    let flags = file.flags();
                    let child_signalfd = file
                        .file_ops()
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
                child_entries.insert(fd, cloned);
            }
            child_entries
        });

        FdTable::from_entries(entries)
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
            BlockType::Futex {
                key,
                bitset,
                deadline_nanos,
            } => {
                match self.take_armed_futex_wait(pid) {
                    Some(armed)
                        if armed.key == key && armed.bitset == bitset && armed.raced_wake =>
                    {
                        process.wake_result = Some(BlockResult::Futex {
                            woke: true,
                            timed_out: false,
                        });
                        process.state = ProcessState::Runnable;
                        return;
                    }
                    Some(_) | None => {}
                }
                ProcessBlock::Futex {
                    key,
                    bitset,
                    deadline_nanos,
                }
            }
            BlockType::SignalSuspend => ProcessBlock::SignalSuspend,
            BlockType::Vfork { child } => ProcessBlock::Vfork { child },
            BlockType::WaitChild {
                selector,
                api,
                status_ptr,
                info_ptr,
                options,
            } => ProcessBlock::WaitChild {
                selector,
                api,
                status_ptr,
                info_ptr,
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
            ProcessBlock::Futex {
                key,
                deadline_nanos,
                ..
            } => {
                self.blocked_futexes.entry(key).or_default().insert(pid);
                if let Some(deadline_nanos) = deadline_nanos {
                    self.blocked_timers
                        .entry(deadline_nanos)
                        .or_default()
                        .insert(pid);
                    self.note_timer_deadline_inserted(deadline_nanos);
                }
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
            ProcessBlock::Futex {
                key,
                deadline_nanos,
                ..
            } => {
                if let Some(waiters) = self.blocked_futexes.get_mut(&key) {
                    waiters.remove(&pid);
                    if waiters.is_empty() {
                        self.blocked_futexes.remove(&key);
                    }
                }
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
            thread_groups: alloc::collections::BTreeMap::new(),
            group_exit_status: alloc::collections::BTreeMap::new(),
            queued_signals: alloc::collections::BTreeMap::new(),
            parent_children: alloc::collections::BTreeMap::new(),
            child_events: alloc::collections::BTreeMap::new(),
            blocked_files: alloc::collections::BTreeSet::new(),
            blocked_timers: alloc::collections::BTreeMap::new(),
            next_timer_deadline_nanos: None,
            blocked_futexes: alloc::collections::BTreeMap::new(),
            armed_futex_waits: alloc::collections::BTreeMap::new(),
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
        let assigned_cpu = crate::processor::select_cpu_for_new_process();
        let identity = ProcessIdentity {
            pid,
            thread_group: pid,
            parent,
            process_group: pid,
            session: pid,
        };
        self.track_child_link(parent, pid);
        self.track_thread_group(pid, pid);

        self.processes.insert(
            pid,
            Box::new(KernelProcess {
                identity,
                controlling_terminal: None,
                pidfd: PidFdHandle::new(pid),
                exit_signal: crate::signal::SIGCHLD,
                task,
                credentials: Credentials::root(),
                prctl: crate::process::PrctlState::for_exec_path(_name),
                assigned_cpu,
                kernel_context: None,
                kernel_cpu: None,
                pending_exec: None,
                pending_syscall: None,
                pending_syscall_name: "",
                pending_file_waits: Vec::new(),
                mmap_regions: Vec::new(),
                vfork_parent: None,
                set_child_tid: None,
                robust_list_head: None,
                robust_list_len: 0,
                clear_child_tid: None,
                files: self.initial_files.fork_copy(),
                fs: self
                    .initial_fs
                    .clone()
                    .ok_or(BuildError::EmptyProgram)?
                    .fork_copy(),
                umask: 0o022,
                signals: SignalState::new(),
                wake_result: None,
                state: ProcessState::Runnable,
            }),
        );
        self.enqueue_runnable_pid(pid, assigned_cpu);
        Ok(pid)
    }

    pub(crate) fn allocate_pid(&mut self, requested: Option<Pid>) -> SysResult<Pid> {
        if let Some(pid) = requested {
            if pid == 0 {
                return Err(crate::errno::SysErr::Inval);
            }
            if self.processes.contains_key(&pid)
                || self.zombies.contains_key(&pid)
                || crate::processor::running_cpu_of(pid).is_some()
            {
                return Err(crate::errno::SysErr::Exists);
            }
            if pid >= self.next_pid {
                self.next_pid = pid.saturating_add(1);
            }
            return Ok(pid);
        }

        let pid = self.next_pid;
        self.next_pid = self.next_pid.saturating_add(1);
        Ok(pid)
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
        self.drain_queued_signals(&mut process);
        if process.assigned_cpu != current_cpu {
            let assigned_cpu = process.assigned_cpu;
            self.processes.insert(pid, process);
            self.enqueue_runnable_pid(pid, assigned_cpu);
            return None;
        }

        if let Some(status) = self
            .group_exit_status
            .get(&process.identity.thread_group)
            .copied()
        {
            process.state = ProcessState::Exited(status);
            self.reap_exited_process(&mut process, status);
            return Some(DispatchWork::Event(ScheduleEvent::Exited { pid, status }));
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
                                    let status = 128 + info.signal as i32;
                                    if process.is_thread() {
                                        self.begin_thread_group_exit(
                                            process.identity.thread_group,
                                            pid,
                                            status,
                                        );
                                    }
                                    process.state = ProcessState::Exited(status);
                                }
                            }
                        }
                    } else {
                        let status = 128 + info.signal as i32;
                        if process.is_thread() {
                            self.begin_thread_group_exit(
                                process.identity.thread_group,
                                pid,
                                status,
                            );
                        }
                        process.state = ProcessState::Exited(status);
                    }
                }
                SignalDelivery::Exit(info) => {
                    let status = 128 + info.signal as i32;
                    if process.is_thread() {
                        self.begin_thread_group_exit(process.identity.thread_group, pid, status);
                    }
                    process.state = ProcessState::Exited(status);
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
            self.reap_exited_process(&mut process, status);
            return Some(DispatchWork::Event(ScheduleEvent::Exited { pid, status }));
        }

        if process.pending_syscall.is_some() {
            process.state = ProcessState::Running;
            return Some(DispatchWork::KernelSyscall(process));
        }

        self.prepare_child_return(&mut process);
        if let ProcessState::Exited(status) = process.state {
            self.reap_exited_process(&mut process, status);
            return Some(DispatchWork::Event(ScheduleEvent::Exited { pid, status }));
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
        self.drain_queued_signals(&mut process);
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
                        let _ = self.queue_synchronous_fault_signal(
                            &mut process,
                            crate::signal::SIGSEGV,
                            error_code as i32,
                        );
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
                        let _ = self.queue_synchronous_fault_signal(
                            &mut process,
                            signal,
                            error_code as i32,
                        );
                        ScheduleEvent::Interrupted { pid, vector }
                    }
                    UserExceptionClass::Fatal(details) => {
                        if let Some(signal) = exception_signal(details.vector) {
                            let _ = self.queue_synchronous_fault_signal(
                                &mut process,
                                signal,
                                error_code as i32,
                            );
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
                self.disarm_futex_wait(pid);
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
                self.disarm_futex_wait(pid);
                process.pending_syscall = None;
                process.state = ProcessState::Exited(status);
                ScheduleEvent::Exited { pid, status }
            }
            SyscallDisposition::ExitGroup(status) => {
                self.disarm_futex_wait(pid);
                process.pending_syscall = None;
                self.begin_thread_group_exit(process.identity.thread_group, pid, status);
                process.state = ProcessState::Exited(status);
                ScheduleEvent::Exited { pid, status }
            }
            SyscallDisposition::Block(block) => {
                if !matches!(block, BlockType::Futex { .. }) {
                    self.disarm_futex_wait(pid);
                }
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
                self.reap_exited_process(&mut process, status);
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
        self.track_thread_group(process.identity.thread_group, pid);
        if !process.is_thread() {
            self.track_child_link(process.identity.parent, pid);
        }
        self.processes.insert(pid, Box::new(process));
        self.enqueue_runnable_pid(pid, assigned_cpu);
        pid
    }

    fn track_thread_group(&mut self, tgid: Pid, pid: Pid) {
        self.thread_groups.entry(tgid).or_default().insert(pid);
    }

    fn untrack_thread_group(&mut self, tgid: Pid, pid: Pid) {
        let Some(group) = self.thread_groups.get_mut(&tgid) else {
            return;
        };
        group.remove(&pid);
        if group.is_empty() {
            self.thread_groups.remove(&tgid);
            self.group_exit_status.remove(&tgid);
        }
    }

    fn begin_thread_group_exit(&mut self, tgid: Pid, current_pid: Pid, status: i32) {
        self.group_exit_status.insert(tgid, status);

        let Some(members) = self.thread_groups.get(&tgid).cloned() else {
            return;
        };

        for pid in members {
            if pid == current_pid {
                continue;
            }

            let mut unblock_block = None;
            let mut queue_assigned_cpu = None;
            if let Some(process) = self.processes.get_mut(&pid) {
                if matches!(process.state, ProcessState::Exited(_)) {
                    continue;
                }

                if matches!(process.state, ProcessState::Blocked(_)) {
                    process.wake_result = Some(BlockResult::SignalInterrupted);
                    unblock_block = Some(process.state);
                }
                process.state = ProcessState::Runnable;
                queue_assigned_cpu = Some(process.assigned_cpu);
            }

            if let Some(assigned_cpu) = queue_assigned_cpu {
                self.enqueue_runnable_pid(pid, assigned_cpu);
            }
            if let Some(blocked_state) = unblock_block {
                self.untrack_blocked_process(pid, blocked_state);
            }
        }
    }

    fn finish_process_exit(&mut self, process: &mut KernelProcess) {
        self.disarm_futex_wait(process.identity.pid);
        self.cleanup_robust_list(process);

        let Some(clear_child_tid) = process.clear_child_tid.take() else {
            return;
        };

        let zero = 0u32.to_ne_bytes();
        if let Ok(written) = process.task.address_space.write(clear_child_tid, &zero)
            && written == zero.len()
        {
            self.wake_futex_scopes(process, clear_child_tid, i32::MAX as usize, u32::MAX);
        }
    }

    fn child_process_group(&self, pid: Pid) -> Option<Pid> {
        self.processes
            .get(&pid)
            .map(|process| process.identity.process_group)
            .or_else(|| self.zombies.get(&pid).map(|zombie| zombie.process_group))
            .or_else(|| crate::processor::running_process_group_of(pid))
    }

    fn child_matches_wait_selector(&self, selector: WaitChildSelector, child_pid: Pid) -> bool {
        match selector {
            WaitChildSelector::Any => true,
            WaitChildSelector::Pid(pid) => child_pid == pid,
            WaitChildSelector::ProcessGroup(process_group) => {
                self.child_process_group(child_pid) == Some(process_group)
            }
        }
    }

    pub(crate) fn wait_child_event(
        &mut self,
        parent_pid: Pid,
        selector: WaitChildSelector,
        options: u64,
        consume: bool,
    ) -> Option<ChildEvent> {
        let index = {
            let events = self.child_events.get(&parent_pid)?;
            events.iter().position(|event| {
                self.child_matches_wait_selector(selector, event.pid)
                    && super::util::child_event_matches_options(event.kind, options)
            })?
        };
        let event = if consume {
            let events = self.child_events.get_mut(&parent_pid)?;
            let event = events.remove(index)?;
            if events.is_empty() {
                self.child_events.remove(&parent_pid);
            }
            event
        } else {
            let events = self.child_events.get(&parent_pid)?;
            *events.get(index)?
        };
        if consume && matches!(event.kind, ChildEventKind::Exited(_)) {
            let zombie = self.zombies.remove(&event.pid);
            if let Some(zombie) = zombie.as_ref() {
                zombie.pidfd.mark_reaped();
            }
            let parent = zombie.and_then(|zombie| zombie.parent);
            self.untrack_child_link(parent, event.pid);
        }
        Some(event)
    }

    pub(crate) fn has_waitable_child(&self, parent_pid: Pid, selector: WaitChildSelector) -> bool {
        let Some(children) = self.parent_children.get(&parent_pid) else {
            return false;
        };
        children
            .iter()
            .copied()
            .any(|child_pid| self.child_matches_wait_selector(selector, child_pid))
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
                    ProcessState::Blocked(ProcessBlock::Futex { .. }) => {
                        self.wake_process_with_result(
                            pid,
                            BlockResult::Futex {
                                woke: false,
                                timed_out: true,
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
            let signal_deliverable = process
                .signals
                .has_deliverable(crate::arch::supports_user_handlers());

            if signal_deliverable
                && matches!(
                    process.state,
                    ProcessState::Blocked(ProcessBlock::SignalSuspend)
                )
            {
                process.signals.leave_sigsuspend();
                process.wake_result = Some(BlockResult::SignalInterrupted);
                process.state = ProcessState::Runnable;
                queue_assigned_cpu = Some(process.assigned_cpu);
            }

            if signal_deliverable
                && let ProcessState::Blocked(ProcessBlock::Timer {
                    target_nanos,
                    request_nanos,
                    rmtp,
                    flags,
                }) = process.state
            {
                let _ = request_nanos;
                let current_nanos = time::monotonic_nanos();
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

            if signal_deliverable
                && let ProcessState::Blocked(ProcessBlock::Poll { .. }) = process.state
            {
                process.wake_result = Some(BlockResult::SignalInterrupted);
                unblock_block = Some(process.state);
                process.state = ProcessState::Runnable;
                queue_assigned_cpu = Some(process.assigned_cpu);
            }

            if signal_deliverable
                && let ProcessState::Blocked(ProcessBlock::Futex { .. }) = process.state
            {
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
        } else if self.queue_signal_for_running(pid, info) {
            true
        } else {
            false
        }
    }

    pub(crate) fn thread_group_of(&self, pid: Pid) -> Option<Pid> {
        self.processes
            .get(&pid)
            .map(|process| process.identity.thread_group)
            .or_else(|| crate::processor::running_thread_group_of(pid))
    }

    pub(crate) fn has_thread_group(&self, tgid: Pid) -> bool {
        self.thread_groups.contains_key(&tgid)
    }

    pub(crate) fn has_process_group(&self, process_group: Pid) -> bool {
        self.thread_groups
            .keys()
            .copied()
            .any(|tgid| self.thread_group_process_group(tgid) == Some(process_group))
    }

    pub(crate) fn setpgid(
        &mut self,
        caller: &mut KernelProcess,
        target_pid: Pid,
        process_group: Pid,
    ) -> SysResult<u64> {
        let target_pid = if target_pid == 0 {
            caller.identity.pid
        } else {
            target_pid
        };
        let process_group = if process_group == 0 {
            target_pid
        } else {
            process_group
        };

        if target_pid == caller.identity.pid {
            if caller.identity.session == caller.identity.pid {
                return Err(crate::errno::SysErr::Perm);
            }
            if process_group != target_pid
                && process_group != caller.identity.process_group
                && self.process_group_session(process_group) != Some(caller.identity.session)
            {
                return Err(crate::errno::SysErr::Perm);
            }

            caller.identity.process_group = process_group;

            // TODO: If CLONE_THREAD is exercised heavily, running sibling threads outside the
            // manager map still need synchronized process-group updates.
            if let Some(members) = self
                .thread_groups
                .get(&caller.identity.thread_group)
                .cloned()
            {
                for pid in members {
                    if pid == caller.identity.pid {
                        continue;
                    }
                    if let Some(process) = self.processes.get_mut(&pid) {
                        process.identity.process_group = process_group;
                    }
                }
            }
            return Ok(0);
        }

        let target_tgid = self
            .thread_group_of(target_pid)
            .ok_or(crate::errno::SysErr::Srch)?;
        let is_child = self
            .parent_children
            .get(&caller.identity.pid)
            .is_some_and(|children| {
                children.contains(&target_tgid) || children.contains(&target_pid)
            });
        if !is_child {
            return Err(crate::errno::SysErr::Srch);
        }

        let Some(members) = self.thread_groups.get(&target_tgid).cloned() else {
            return Err(crate::errno::SysErr::Srch);
        };

        let Some(representative) = members.iter().find_map(|pid| self.processes.get(pid)) else {
            // TODO: Linux allows setpgid() on an eligible child even if it is currently
            // running. Supporting that here needs scheduler-visible mutable process metadata.
            return Err(crate::errno::SysErr::Srch);
        };

        if representative.identity.session != caller.identity.session {
            return Err(crate::errno::SysErr::Perm);
        }
        if representative.identity.session == target_tgid {
            return Err(crate::errno::SysErr::Perm);
        }
        if process_group != target_tgid
            && process_group != caller.identity.process_group
            && self.process_group_session(process_group) != Some(caller.identity.session)
        {
            return Err(crate::errno::SysErr::Perm);
        }

        // TODO: Linux returns EACCES if the child already passed through execve(). We do not
        // track that state separately yet, so this check is still missing.
        for pid in members {
            let Some(process) = self.processes.get_mut(&pid) else {
                return Err(crate::errno::SysErr::Srch);
            };
            process.identity.process_group = process_group;
        }
        Ok(0)
    }

    pub(crate) fn send_signal_thread_group(&mut self, tgid: Pid, info: SignalInfo) -> bool {
        let Some(target) = self.pick_thread_group_target(tgid, info.signal) else {
            return false;
        };
        self.send_signal(target, info)
    }

    pub(crate) fn send_signal_process_group(
        &mut self,
        process_group: Pid,
        info: SignalInfo,
    ) -> usize {
        let targets = self
            .thread_groups
            .keys()
            .copied()
            .filter(|tgid| self.thread_group_process_group(*tgid) == Some(process_group))
            .collect::<Vec<_>>();
        let mut delivered = 0usize;
        for tgid in targets {
            delivered += usize::from(self.send_signal_thread_group(tgid, info));
        }
        delivered
    }

    pub(crate) fn send_signal_all_thread_groups(
        &mut self,
        info: SignalInfo,
        exclude_tgid: Option<Pid>,
    ) -> usize {
        let targets = self
            .thread_groups
            .keys()
            .copied()
            .filter(|tgid| *tgid != 1 && Some(*tgid) != exclude_tgid)
            .collect::<Vec<_>>();
        let mut delivered = 0usize;
        for tgid in targets {
            delivered += usize::from(self.send_signal_thread_group(tgid, info));
        }
        delivered
    }

    pub(crate) fn wake_futex(
        &mut self,
        key: crate::process::FutexKey,
        bitset: u32,
        count: usize,
    ) -> usize {
        let mut woke = self.wake_armed_futex_waits(key, bitset, count);
        if woke >= count {
            return woke;
        }

        let Some(waiters) = self.blocked_futexes.get(&key).cloned() else {
            return woke;
        };
        for pid in waiters {
            if woke >= count {
                break;
            }
            let Some(state) = self.processes.get(&pid).map(|process| process.state) else {
                continue;
            };
            let ProcessState::Blocked(ProcessBlock::Futex {
                key: wait_key,
                bitset: wait_bitset,
                ..
            }) = state
            else {
                continue;
            };
            if wait_key != key || (wait_bitset & bitset) == 0 {
                continue;
            }
            self.untrack_blocked_process(pid, state);
            let Some(process) = self.processes.get_mut(&pid) else {
                continue;
            };
            process.wake_result = Some(BlockResult::Futex {
                woke: true,
                timed_out: false,
            });
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
        from: crate::process::FutexKey,
        to: crate::process::FutexKey,
        wake_count: usize,
        requeue_count: usize,
        bitset: u32,
    ) -> usize {
        let woke = self.wake_futex(from, bitset, wake_count);
        let mut moved = 0usize;
        moved += self.requeue_armed_futex_waits(from, to, requeue_count, bitset);
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
                key,
                bitset: wait_bitset,
                deadline_nanos,
            }) = state
            else {
                continue;
            };
            if key != from || (wait_bitset & bitset) == 0 {
                continue;
            }
            self.untrack_blocked_process(pid, state);
            let new_state = {
                let Some(process) = self.processes.get_mut(&pid) else {
                    continue;
                };
                process.state = ProcessState::Blocked(ProcessBlock::Futex {
                    key: to,
                    bitset: wait_bitset,
                    deadline_nanos,
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
        let mut preserve_event = false;
        let blocked_wait = self.processes.get(&parent_pid).and_then(|parent_process| {
            let ProcessState::Blocked(ProcessBlock::WaitChild {
                selector,
                api,
                status_ptr,
                info_ptr: _,
                options,
            }) = parent_process.state
            else {
                return None;
            };
            Some((selector, api, status_ptr, options))
        });
        if let Some((selector, api, status_ptr, options)) = blocked_wait
            && self.child_matches_wait_selector(selector, event.pid)
            && super::util::child_event_matches_options(event.kind, options)
        {
            let nohang = (options & 1) != 0;
            if !nohang {
                let Some(parent_process) = self.processes.get_mut(&parent_pid) else {
                    return;
                };
                preserve_event = matches!(api, WaitChildApi::WaitId);
                if matches!(api, WaitChildApi::Wait4) && status_ptr != 0 {
                    let raw = super::util::wait_status(event.kind).to_ne_bytes();
                    let _ = parent_process.task.address_space.write(status_ptr, &raw);
                }
                parent_process.wake_result = Some(BlockResult::CompletedValue {
                    value: if matches!(api, WaitChildApi::Wait4) {
                        event.pid as u64
                    } else {
                        0
                    },
                });
                parent_process.state = ProcessState::Runnable;
                let assigned_cpu = parent_process.assigned_cpu;
                let _ = parent_process;
                self.enqueue_runnable_pid(parent_pid, assigned_cpu);
                if matches!(api, WaitChildApi::Wait4)
                    && matches!(event.kind, ChildEventKind::Exited(_))
                {
                    let parent = self
                        .zombies
                        .remove(&event.pid)
                        .and_then(|zombie| zombie.parent);
                    self.untrack_child_link(parent, event.pid);
                }
                woke_waiter = true;
            }
        }

        if !woke_waiter || preserve_event {
            self.child_events
                .entry(parent_pid)
                .or_default()
                .push_back(event);
        }

        let sigchld_action = self.parent_sigchld_action(Some(parent_pid));
        let no_cldstop = sigchld_action
            .map(|action| (action.flags & SA_NOCLDSTOP) != 0)
            .unwrap_or(false);

        match event.kind {
            ChildEventKind::Exited(_status) => {}
            ChildEventKind::Stopped(signal) => {
                if !no_cldstop {
                    self.notify_signal(parent_pid, SignalInfo::child_stop(event.pid, signal));
                }
            }
            ChildEventKind::Continued => {
                if !no_cldstop {
                    self.notify_signal(parent_pid, SignalInfo::child_continue(event.pid));
                }
            }
        }
    }
}

impl Default for ProcessManager {
    fn default() -> Self {
        Self::new()
    }
}
