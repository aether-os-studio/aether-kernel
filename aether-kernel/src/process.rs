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

use aether_frame::libs::spin::{LocalIrqDisabled, SpinLock};
use aether_frame::mm::MapFlags;
use aether_frame::process::KernelContext;
use aether_process::BuiltProcess;
use aether_vfs::{NodeRef, PollEvents, SharedOpenFile};

use crate::credentials::Credentials;
use crate::errno::SysResult;
use crate::fs::{FdTable, FileSystemIdentity, LinuxStatFs, PidFdHandle};
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
    pub thread_group: Pid,
    pub parent: Option<Pid>,
    pub process_group: Pid,
    pub session: Pid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FutexScope {
    Private { address_space_id: usize },
    Shared,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct FutexKey {
    pub scope: FutexScope,
    pub uaddr: u64,
}

impl FutexKey {
    pub const fn private(address_space_id: usize, uaddr: u64) -> Self {
        Self {
            scope: FutexScope::Private { address_space_id },
            uaddr,
        }
    }

    pub const fn shared(uaddr: u64) -> Self {
        Self {
            scope: FutexScope::Shared,
            uaddr,
        }
    }
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
        selector: WaitChildSelector,
        api: WaitChildApi,
        status_ptr: u64,
        info_ptr: u64,
        options: u64,
    },
    Futex {
        key: FutexKey,
        bitset: u32,
        deadline_nanos: Option<u64>,
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
    pub controlling_terminal: Option<u64>,
    pub pidfd: Arc<PidFdHandle>,
    pub exit_signal: u8,
    pub task: BuiltProcess,
    pub credentials: Credentials,
    pub prctl: PrctlState,
    pub assigned_cpu: usize,
    pub kernel_context: Option<KernelContext>,
    pub kernel_cpu: Option<usize>,
    pub pending_exec: Option<BuiltProcess>,
    pub pending_syscall: Option<PendingSyscall>,
    pub pending_syscall_name: &'static str,
    pub pending_file_waits: Vec<PendingPollRegistration>,
    pub mmap_regions: Vec<MmapRegion>,
    pub vfork_parent: Option<Pid>,
    pub set_child_tid: Option<u64>,
    pub robust_list_head: Option<u64>,
    pub robust_list_len: u64,
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

#[derive(Clone)]
pub enum MmapRegionBacking {
    Anonymous,
    BufferedFile { file: SharedOpenFile, offset: u64 },
    SharedFile { file: SharedOpenFile, offset: u64 },
    DirectFile { file: SharedOpenFile, offset: u64 },
}

#[derive(Clone)]
pub struct MmapRegion {
    pub start: u64,
    pub end: u64,
    pub page_flags: MapFlags,
    pub mmap_flags: u64,
    pub backing: MmapRegionBacking,
}

impl KernelProcess {
    pub const fn is_thread(&self) -> bool {
        self.identity.thread_group != self.identity.pid
    }

    pub(crate) fn running_snapshot(&self) -> RunningProcessSnapshot {
        RunningProcessSnapshot {
            pid: self.identity.pid,
            thread_group: self.identity.thread_group,
            parent: self.identity.parent,
            process_group: self.identity.process_group,
            name: *self.prctl.name_bytes(),
            credentials: self.credentials.clone(),
            umask: self.umask,
        }
    }

    fn mmap_backing_is_mergeable(lhs: &MmapRegion, rhs: &MmapRegion) -> bool {
        if lhs.end != rhs.start
            || lhs.page_flags != rhs.page_flags
            || lhs.mmap_flags != rhs.mmap_flags
        {
            return false;
        }

        match (&lhs.backing, &rhs.backing) {
            (MmapRegionBacking::Anonymous, MmapRegionBacking::Anonymous) => true,
            (
                MmapRegionBacking::BufferedFile {
                    file: lhs_file,
                    offset: lhs_offset,
                },
                MmapRegionBacking::BufferedFile {
                    file: rhs_file,
                    offset: rhs_offset,
                },
            )
            | (
                MmapRegionBacking::SharedFile {
                    file: lhs_file,
                    offset: lhs_offset,
                },
                MmapRegionBacking::SharedFile {
                    file: rhs_file,
                    offset: rhs_offset,
                },
            )
            | (
                MmapRegionBacking::DirectFile {
                    file: lhs_file,
                    offset: lhs_offset,
                },
                MmapRegionBacking::DirectFile {
                    file: rhs_file,
                    offset: rhs_offset,
                },
            ) => {
                Arc::ptr_eq(lhs_file, rhs_file)
                    && rhs_offset.saturating_sub(*lhs_offset) == lhs.end.saturating_sub(lhs.start)
            }
            _ => false,
        }
    }

    fn coalesce_mmap_regions(&mut self) {
        if self.mmap_regions.len() < 2 {
            return;
        }

        let mut merged: Vec<MmapRegion> = Vec::with_capacity(self.mmap_regions.len());
        for region in self.mmap_regions.drain(..) {
            if let Some(previous) = merged.last_mut()
                && Self::mmap_backing_is_mergeable(previous, &region)
            {
                previous.end = region.end;
                continue;
            }
            merged.push(region);
        }
        self.mmap_regions = merged;
    }

    pub fn covering_mmap_region(&self, start: u64, end: u64) -> Option<&MmapRegion> {
        self.mmap_regions
            .iter()
            .find(|region| start >= region.start && end <= region.end)
    }

    pub fn slice_mmap_region(&self, start: u64, end: u64) -> Option<MmapRegion> {
        let region = self.covering_mmap_region(start, end)?.clone();
        Some(MmapRegion {
            start,
            end,
            page_flags: region.page_flags,
            mmap_flags: region.mmap_flags,
            backing: Self::adjusted_mmap_backing(
                &region.backing,
                start.saturating_sub(region.start),
            ),
        })
    }

    fn adjusted_mmap_backing(backing: &MmapRegionBacking, delta: u64) -> MmapRegionBacking {
        match backing {
            MmapRegionBacking::Anonymous => MmapRegionBacking::Anonymous,
            MmapRegionBacking::BufferedFile { file, offset } => MmapRegionBacking::BufferedFile {
                file: file.clone(),
                offset: offset.saturating_add(delta),
            },
            MmapRegionBacking::SharedFile { file, offset } => MmapRegionBacking::SharedFile {
                file: file.clone(),
                offset: offset.saturating_add(delta),
            },
            MmapRegionBacking::DirectFile { file, offset } => MmapRegionBacking::DirectFile {
                file: file.clone(),
                offset: offset.saturating_add(delta),
            },
        }
    }

    pub fn insert_mmap_region(&mut self, region: MmapRegion) {
        let index = self
            .mmap_regions
            .binary_search_by_key(&region.start, |existing| existing.start)
            .unwrap_or_else(|index| index);
        self.mmap_regions.insert(index, region);
        self.coalesce_mmap_regions();
    }

    pub fn update_mmap_region_flags(&mut self, start: u64, end: u64, page_flags: MapFlags) {
        let mut updated = Vec::with_capacity(self.mmap_regions.len().saturating_add(2));
        for region in self.mmap_regions.drain(..) {
            if region.end <= start || region.start >= end {
                updated.push(region);
                continue;
            }

            if start > region.start {
                updated.push(MmapRegion {
                    start: region.start,
                    end: start,
                    ..region.clone()
                });
            }

            updated.push(MmapRegion {
                start: region.start.max(start),
                end: region.end.min(end),
                page_flags,
                mmap_flags: region.mmap_flags,
                backing: Self::adjusted_mmap_backing(
                    &region.backing,
                    region.start.max(start).saturating_sub(region.start),
                ),
            });

            if end < region.end {
                updated.push(MmapRegion {
                    start: end,
                    end: region.end,
                    page_flags: region.page_flags,
                    mmap_flags: region.mmap_flags,
                    backing: Self::adjusted_mmap_backing(
                        &region.backing,
                        end.saturating_sub(region.start),
                    ),
                });
            }
        }
        self.mmap_regions = updated;
        self.coalesce_mmap_regions();
    }

    pub fn remove_mmap_region_range(&mut self, start: u64, end: u64) {
        let mut updated = Vec::with_capacity(self.mmap_regions.len());
        for region in self.mmap_regions.drain(..) {
            if end <= region.start || start >= region.end {
                updated.push(region);
                continue;
            }
            if start > region.start {
                updated.push(MmapRegion {
                    start: region.start,
                    end: start,
                    ..region.clone()
                });
            }
            if end < region.end {
                updated.push(MmapRegion {
                    start: end,
                    end: region.end,
                    page_flags: region.page_flags,
                    mmap_flags: region.mmap_flags,
                    backing: Self::adjusted_mmap_backing(
                        &region.backing,
                        end.saturating_sub(region.start),
                    ),
                });
            }
        }
        self.mmap_regions = updated;
        self.coalesce_mmap_regions();
    }
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
pub enum ChildEventKind {
    Exited(i32),
    Stopped(u8),
    Continued,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChildEvent {
    pub pid: Pid,
    pub kind: ChildEventKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitChildSelector {
    Any,
    Pid(Pid),
    ProcessGroup(Pid),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitChildApi {
    Wait4,
    WaitId,
}

#[derive(Clone)]
struct ZombieProcess {
    parent: Option<Pid>,
    process_group: Pid,
    pidfd: Arc<PidFdHandle>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RunningProcessSnapshot {
    pub pid: Pid,
    pub thread_group: Pid,
    pub parent: Option<Pid>,
    pub process_group: Pid,
    pub name: [u8; 16],
    pub credentials: Credentials,
    pub umask: u16,
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
    fn link(
        &mut self,
        fs: &ProcessFsContext,
        old_path: &str,
        new_path: &str,
        flags: u64,
    ) -> SysResult<u64>;
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
    fn wait_child_event(
        &mut self,
        parent_pid: Pid,
        selector: WaitChildSelector,
        options: u64,
        consume: bool,
    ) -> Option<ChildEvent>;
    fn has_waitable_child(&mut self, parent_pid: Pid, selector: WaitChildSelector) -> bool;
    fn thread_group_of(&mut self, pid: Pid) -> Option<Pid>;
    fn has_thread_group(&mut self, tgid: Pid) -> bool;
    fn has_process_group(&mut self, process_group: Pid) -> bool;
    fn process_group_session(&mut self, process_group: Pid) -> Option<Pid>;
    fn setpgid(
        &mut self,
        caller: &mut KernelProcess,
        pid: Pid,
        process_group: Pid,
    ) -> SysResult<u64>;
    fn wake_vfork_parent(&mut self, parent_pid: Pid, child_pid: Pid);
    fn send_kernel_signal(&mut self, pid: Pid, signal: crate::signal::SignalInfo) -> bool;
    fn send_process_signal(&mut self, pid: Pid, signal: crate::signal::SignalInfo) -> bool;
    fn send_process_group_signal(
        &mut self,
        process_group: Pid,
        signal: crate::signal::SignalInfo,
    ) -> usize;
    fn send_signal_all(
        &mut self,
        signal: crate::signal::SignalInfo,
        exclude_tgid: Option<Pid>,
    ) -> usize;
    fn arm_futex_wait(&mut self, pid: Pid, key: FutexKey, bitset: u32);
    fn disarm_futex_wait(&mut self, pid: Pid);
    fn wake_futex(&mut self, key: FutexKey, bitset: u32, count: usize) -> usize;
    fn requeue_futex(
        &mut self,
        from: FutexKey,
        to: FutexKey,
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
    processes: BTreeMap<Pid, ProcessBox>,
    zombies: BTreeMap<Pid, ZombieProcess>,
    thread_groups: BTreeMap<Pid, BTreeSet<Pid>>,
    group_exit_status: BTreeMap<Pid, i32>,
    queued_signals: BTreeMap<Pid, VecDeque<crate::signal::SignalInfo>>,
    parent_children: BTreeMap<Pid, BTreeSet<Pid>>,
    child_events: BTreeMap<Pid, VecDeque<ChildEvent>>,
    blocked_files: BTreeSet<Pid>,
    blocked_timers: BTreeMap<u64, BTreeSet<Pid>>,
    next_timer_deadline_nanos: Option<u64>,
    blocked_futexes: BTreeMap<FutexKey, BTreeSet<Pid>>,
    armed_futex_waits: BTreeMap<Pid, ArmedFutexWait>,
    file_wait_queue: Arc<SpinLock<VecDeque<Pid>, LocalIrqDisabled>>,
    file_wait_enqueued: Arc<SpinLock<BTreeSet<Pid>, LocalIrqDisabled>>,
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
struct ArmedFutexWait {
    key: FutexKey,
    bitset: u32,
    raced_wake: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub struct ProcessStateSnapshot {
    pub pid: Pid,
    pub state: ProcessState,
}
