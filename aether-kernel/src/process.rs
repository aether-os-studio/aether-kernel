extern crate alloc;

mod clone;
mod context;
mod manager;
mod util;

use alloc::boxed::Box;
use alloc::collections::btree_set::BTreeSet;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::AtomicBool;

use aether_frame::libs::spin::SpinLock;
use aether_frame::process::KernelContext;
use aether_process::BuiltProcess;
use aether_vfs::{NodeRef, PollEvents, SharedOpenFile};

use crate::credentials::Credentials;
use crate::errno::SysResult;
use crate::fs::{FdTable, FileSystemIdentity, LinuxStatFs};
use crate::rootfs::ProcessFsContext;
use crate::signal::SignalState;
use crate::syscall::{BlockResult, SyscallArgs};

pub type Pid = u32;
pub type ProcessBox = Box<KernelProcess>;
pub(crate) use self::clone::{CloneParams, LinuxCloneArgs};
pub(crate) use self::context::ProcessSyscallContext;
pub(crate) use self::context::fd::resolve_at_path;
pub(crate) use self::util::anonymous_filesystem_identity;
pub(crate) use self::util::decode_sigset;
pub(crate) use self::util::read_iovec_array;
pub(crate) use self::util::wait_status;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessIdentity {
    pub pid: Pid,
    pub parent: Option<Pid>,
    pub process_group: Pid,
    pub session: Pid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessState {
    Runnable,
    Running,
    Stopped(u8),
    Blocked(ProcessBlock),
    Exited(i32),
    Faulted { vector: u8, error_code: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessBlock {
    File {
        fd: u32,
        events: PollEvents,
    },
    Poll {
        deadline_nanos: Option<u64>,
    },
    SignalSuspend,
    Vfork {
        child: Pid,
    },
    WaitChild {
        pid: i32,
        status_ptr: u64,
        options: u64,
    },
    Futex {
        uaddr: u64,
        bitset: u32,
    },
    Timer {
        target_nanos: u64,
        request_nanos: u64,
        rmtp: u64,
        flags: u64,
    },
}

pub struct KernelProcess {
    pub identity: ProcessIdentity,
    pub task: BuiltProcess,
    pub credentials: Credentials,
    pub prctl: PrctlState,
    pub assigned_cpu: usize,
    pub kernel_context: Option<KernelContext>,
    pub kernel_cpu: Option<usize>,
    pub pending_exec: Option<BuiltProcess>,
    pub pending_syscall: Option<PendingSyscall>,
    pub pending_syscall_name: &'static str,
    pub pending_poll: Option<PendingPollState>,
    pub vfork_parent: Option<Pid>,
    pub clear_child_tid: Option<u64>,
    pub files: FdTable,
    pub fs: ProcessFsContext,
    pub umask: u16,
    pub signals: SignalState,
    pub wake_result: Option<BlockResult>,
    pub state: ProcessState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingPollRegistration {
    pub fd: u32,
    pub events: PollEvents,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingPollFd {
    pub fd: i32,
    pub events: i16,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PendingPollState {
    pub user_fds: u64,
    pub deadline_nanos: Option<u64>,
    pub items: Vec<PendingPollFd>,
    pub registrations: Vec<PendingPollRegistration>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrctlState {
    pub parent_death_signal: u8,
    pub dumpable: bool,
    pub keepcaps: bool,
    pub no_new_privs: bool,
    pub child_subreaper: bool,
    pub timer_slack_nanos: u64,
    pub thp_disable: bool,
    pub capability_bounding_set: u64,
    pub name: [u8; 16],
}

impl PrctlState {
    pub fn for_exec_path(path: &str) -> Self {
        const CAP_LAST_CAP: u64 = 40;

        let mut state = Self {
            parent_death_signal: 0,
            dumpable: true,
            keepcaps: false,
            no_new_privs: false,
            child_subreaper: false,
            timer_slack_nanos: 50_000,
            thp_disable: false,
            capability_bounding_set: if CAP_LAST_CAP >= 63 {
                u64::MAX
            } else {
                (1u64 << (CAP_LAST_CAP + 1)) - 1
            },
            name: [0; 16],
        };
        state.set_name(path);
        state
    }

    pub fn set_name(&mut self, path: &str) {
        let candidate = path.rsplit('/').next().unwrap_or(path);
        let bytes = candidate.as_bytes();
        let len = bytes.len().min(15);
        self.name = [0; 16];
        self.name[..len].copy_from_slice(&bytes[..len]);
    }

    pub fn set_name_bytes(&mut self, bytes: &[u8]) {
        let len = bytes
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(bytes.len())
            .min(15);
        self.name = [0; 16];
        self.name[..len].copy_from_slice(&bytes[..len]);
    }

    pub const fn name_bytes(&self) -> &[u8; 16] {
        &self.name
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScheduleEvent {
    #[allow(dead_code)]
    Idle,
    Syscall {
        pid: Pid,
        number: u64,
        name: &'static str,
    },
    Interrupted {
        pid: Pid,
        vector: u8,
    },
    Exited {
        pid: Pid,
        status: i32,
    },
    Faulted {
        pid: Pid,
        vector: u8,
        error_code: u64,
    },
}

pub enum DispatchWork {
    Idle,
    Event(ScheduleEvent),
    Process(ProcessBox),
    KernelSyscall(ProcessBox),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingSyscall {
    pub number: u64,
    pub args: SyscallArgs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChildEventKind {
    Exited(i32),
    Stopped(u8),
    Continued,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ChildEvent {
    pub pid: Pid,
    pub kind: ChildEventKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ZombieProcess {
    parent: Option<Pid>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RunningProcess {
    parent: Option<Pid>,
    name: [u8; 16],
    credentials: Credentials,
    umask: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcFsProcessSnapshot {
    pub pid: Pid,
    pub parent: Option<Pid>,
    pub state: ProcessState,
    pub name: [u8; 16],
    pub credentials: Credentials,
    pub umask: u16,
}

pub trait ProcessServices {
    fn lookup_node_with_identity(
        &mut self,
        fs: &ProcessFsContext,
        path: &str,
        follow_final: bool,
    ) -> SysResult<(NodeRef, FileSystemIdentity)>;
    fn statfs(&mut self, fs: &ProcessFsContext, path: &str) -> SysResult<LinuxStatFs>;
    fn mkdir(&mut self, fs: &ProcessFsContext, path: &str, mode: u32) -> SysResult<u64>;
    fn create_file(
        &mut self,
        fs: &ProcessFsContext,
        path: &str,
        mode: u32,
    ) -> SysResult<(NodeRef, FileSystemIdentity)>;
    fn create_symlink(&mut self, fs: &ProcessFsContext, path: &str, target: &str)
    -> SysResult<u64>;
    fn bind_socket(&mut self, fs: &ProcessFsContext, path: &str, mode: u32) -> SysResult<u64>;
    fn unlink(&mut self, fs: &ProcessFsContext, path: &str, flags: u64) -> SysResult<u64>;
    fn rename(&mut self, fs: &ProcessFsContext, old_path: &str, new_path: &str) -> SysResult<u64>;
    fn getcwd(&mut self, fs: &ProcessFsContext) -> String;
    fn chdir(&mut self, fs: &mut ProcessFsContext, path: &str) -> SysResult<u64>;
    fn chroot(&mut self, fs: &mut ProcessFsContext, path: &str) -> SysResult<u64>;
    fn mount(
        &mut self,
        fs: &mut ProcessFsContext,
        source: Option<&str>,
        target: &str,
        fstype: Option<&str>,
        flags: u64,
    ) -> SysResult<u64>;
    fn umount(&mut self, fs: &ProcessFsContext, target: &str, flags: u64) -> SysResult<u64>;
    fn pivot_root(
        &mut self,
        fs: &mut ProcessFsContext,
        new_root: &str,
        put_old: &str,
    ) -> SysResult<u64>;
    fn execve(
        &mut self,
        process: &mut KernelProcess,
        path: &str,
        argv: Vec<String>,
        envp: Vec<String>,
    ) -> SysResult<u64>;
    fn clone_process(&mut self, parent: &mut KernelProcess, params: CloneParams) -> SysResult<Pid>;
    fn reap_child_event(
        &mut self,
        parent_pid: Pid,
        requested: i32,
        options: u64,
    ) -> Option<ChildEvent>;
    fn has_child(&mut self, parent_pid: Pid, requested: i32) -> bool;
    fn wake_vfork_parent(&mut self, parent_pid: Pid, child_pid: Pid);
    fn send_kernel_signal(&mut self, pid: Pid, signal: crate::signal::SignalInfo) -> bool;
    fn wake_futex(&mut self, uaddr: u64, bitset: u32, count: usize) -> usize;
    fn requeue_futex(
        &mut self,
        from: u64,
        to: u64,
        wake_count: usize,
        requeue_count: usize,
        bitset: u32,
    ) -> usize;
    fn log_unimplemented(&mut self, number: u64, name: &str, pid: Pid, args: SyscallArgs);
    fn log_unimplemented_command(
        &mut self,
        name: &str,
        command_name: &str,
        command: u64,
        pid: Pid,
        args: SyscallArgs,
    );
}

pub struct ProcessManager {
    next_pid: Pid,
    run_queue: VecDeque<Pid>,
    processes: BTreeMap<Pid, ProcessBox>,
    running: BTreeMap<Pid, RunningProcess>,
    zombies: BTreeMap<Pid, ZombieProcess>,
    parent_children: BTreeMap<Pid, BTreeSet<Pid>>,
    child_events: BTreeMap<Pid, VecDeque<ChildEvent>>,
    blocked_files: BTreeSet<Pid>,
    blocked_timers: BTreeMap<u64, BTreeSet<Pid>>,
    next_timer_deadline_nanos: Option<u64>,
    blocked_futexes: BTreeMap<u64, BTreeSet<Pid>>,
    file_wait_queue: Arc<SpinLock<VecDeque<Pid>>>,
    file_wait_pending: Arc<AtomicBool>,
    file_wait_registrations: BTreeMap<Pid, Vec<FileWaitRegistration>>,
    initial_files: FdTable,
    initial_fs: Option<ProcessFsContext>,
}

struct FileWaitRegistration {
    file: SharedOpenFile,
    waiter_id: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub struct ProcessStateSnapshot {
    pub pid: Pid,
    pub state: ProcessState,
}
