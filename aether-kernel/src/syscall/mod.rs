pub mod abi;
#[allow(dead_code)]
mod context;
mod handlers;
mod registry;

use crate::arch::ArchContext;
use crate::errno::{SysErr, SysResult};
use crate::process::{CloneParams, Pid};
use aether_vfs::PollEvents;
use alloc::string::String;
use alloc::vec::Vec;

pub use self::registry::{SyscallDispatch, SyscallEntry};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyscallArgs {
    raw: [u64; 6],
}

impl SyscallArgs {
    pub fn from_context(context: &impl ArchContext) -> Self {
        Self {
            raw: context.syscall_args(),
        }
    }

    pub fn get(self, index: usize) -> u64 {
        self.raw.get(index).copied().unwrap_or(0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockType {
    Timer {
        target_nanos: u64,
        request_nanos: u64,
        rmtp: u64,
        flags: u64,
    },
    File {
        fd: u32,
        events: PollEvents,
    },
    Poll {
        deadline_nanos: Option<u64>,
    },
    Futex {
        uaddr: u64,
        bitset: u32,
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockResult {
    Timer {
        completed: bool,
        remaining_nanos: u64,
        rmtp: u64,
        is_absolute: bool,
    },
    File {
        ready: bool,
    },
    Poll {
        timed_out: bool,
    },
    Futex {
        woke: bool,
    },
    SignalInterrupted,
    CompletedValue {
        value: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyscallDisposition {
    Return(SysResult<u64>),
    Block(BlockType),
    Exit(i32),
}

impl SyscallDisposition {
    pub fn ok(value: u64) -> Self {
        Self::Return(Ok(value))
    }

    pub fn err(error: SysErr) -> Self {
        Self::Return(Err(error))
    }

    pub fn block(block_type: BlockType) -> Self {
        Self::Block(block_type)
    }
}

#[allow(dead_code)]
pub trait KernelSyscallContext {
    fn pid(&self) -> u32;

    fn take_wake_result(&mut self) -> Option<BlockResult>;

    fn has_wake_result(&self) -> bool;

    fn brk(&mut self, address: u64) -> SysResult<u64>;
    fn mmap(
        &mut self,
        address: u64,
        len: u64,
        prot: u64,
        flags: u64,
        fd: u64,
        offset: u64,
    ) -> SysResult<u64>;
    fn access(&mut self, path: &str, mode: u64) -> SysResult<u64>;
    fn faccessat(&mut self, dirfd: i64, path: &str, mode: u64, flags: u64) -> SysResult<u64>;
    fn munmap(&mut self, address: u64, len: u64) -> SysResult<u64>;
    fn mprotect(&mut self, address: u64, len: u64, prot: u64) -> SysResult<u64>;
    fn mremap(
        &mut self,
        old_address: u64,
        old_size: u64,
        new_size: u64,
        flags: u64,
        new_address: u64,
    ) -> SysResult<u64>;
    fn openat(&mut self, dirfd: i64, path: &str, flags: u64, mode: u64) -> SysResult<u64>;
    fn creat(&mut self, path: &str, mode: u64) -> SysResult<u64>;
    fn link(&mut self, old_path: &str, new_path: &str) -> SysResult<u64>;
    fn linkat(
        &mut self,
        olddirfd: i64,
        old_path: &str,
        newdirfd: i64,
        new_path: &str,
        flags: u64,
    ) -> SysResult<u64>;
    fn symlink(&mut self, target: &str, linkpath: &str) -> SysResult<u64>;
    fn unlinkat(&mut self, dirfd: i64, path: &str, flags: u64) -> SysResult<u64>;
    fn readlinkat(&mut self, dirfd: i64, path: &str, address: u64, len: usize) -> SysResult<u64>;
    fn rename(&mut self, old_path: &str, new_path: &str) -> SysResult<u64>;
    fn renameat(
        &mut self,
        olddirfd: i64,
        old_path: &str,
        newdirfd: i64,
        new_path: &str,
    ) -> SysResult<u64>;
    fn close_fd(&mut self, fd: u64) -> SysResult<u64>;
    fn close_range(&mut self, first: u64, last: u64, flags: u64) -> SysResult<u64>;
    fn lseek(&mut self, fd: u64, offset: i64, whence: u64) -> SysResult<u64>;
    fn read_fd(&mut self, fd: u64, address: u64, len: usize) -> SysResult<u64>;
    fn read_fd_blocking(&mut self, fd: u64, address: u64, len: usize) -> SyscallDisposition;
    fn write_fd(&mut self, fd: u64, address: u64, len: usize) -> SysResult<u64>;
    fn write_fd_blocking(&mut self, fd: u64, address: u64, len: usize) -> SyscallDisposition;
    fn poll(&mut self, fds: u64, nfds: usize, timeout: i32) -> SysResult<u64>;
    fn poll_blocking(&mut self, fds: u64, nfds: usize, timeout: i32) -> SyscallDisposition;
    fn ppoll(
        &mut self,
        fds: u64,
        nfds: usize,
        timeout: u64,
        sigmask: u64,
        sigsetsize: usize,
    ) -> SysResult<u64>;
    fn ppoll_blocking(
        &mut self,
        fds: u64,
        nfds: usize,
        timeout: u64,
        sigmask: u64,
        sigsetsize: usize,
    ) -> SyscallDisposition;
    fn sendfile(&mut self, out_fd: u64, in_fd: u64, offset: u64, count: usize) -> SysResult<u64>;
    fn sendfile_blocking(
        &mut self,
        out_fd: u64,
        in_fd: u64,
        offset: u64,
        count: usize,
    ) -> SyscallDisposition;
    fn pread64(&mut self, fd: u64, address: u64, len: usize, offset: u64) -> SysResult<u64>;
    fn pwrite64(&mut self, fd: u64, address: u64, len: usize, offset: u64) -> SysResult<u64>;
    fn readv_fd(&mut self, fd: u64, iov: u64, iovcnt: usize) -> SysResult<u64>;
    fn readv_fd_blocking(&mut self, fd: u64, iov: u64, iovcnt: usize) -> SyscallDisposition;
    fn writev_fd(&mut self, fd: u64, iov: u64, iovcnt: usize) -> SysResult<u64>;
    fn writev_fd_blocking(&mut self, fd: u64, iov: u64, iovcnt: usize) -> SyscallDisposition;
    fn preadv64(&mut self, fd: u64, iov: u64, iovcnt: usize, offset: u64) -> SysResult<u64>;
    fn pwritev64(&mut self, fd: u64, iov: u64, iovcnt: usize, offset: u64) -> SysResult<u64>;
    fn fadvise64(&mut self, fd: u64, offset: u64, len: u64, advice: u64) -> SysResult<u64>;
    fn fallocate(&mut self, fd: u64, mode: u64, offset: i64, len: i64) -> SysResult<u64>;
    fn ioctl_fd(&mut self, fd: u64, command: u64, argument: u64) -> SysResult<u64>;
    fn flock(&mut self, fd: u64, operation: u64) -> SysResult<u64>;
    fn flock_blocking(&mut self, fd: u64, operation: u64) -> SyscallDisposition;
    fn fcntl(&mut self, fd: u64, command: u64, arg: u64) -> SysResult<u64>;
    fn fstat(&mut self, fd: u64, address: u64) -> SysResult<u64>;
    fn fstatfs(&mut self, fd: u64, address: u64) -> SysResult<u64>;
    fn dup(&mut self, fd: u64) -> SysResult<u64>;
    fn dup2(&mut self, oldfd: u64, newfd: u64) -> SysResult<u64>;
    fn dup3(&mut self, oldfd: u64, newfd: u64, flags: u64) -> SysResult<u64>;
    fn socket(&mut self, domain: i32, socket_type: u64, protocol: i32) -> SysResult<u64>;
    fn setsockopt(
        &mut self,
        fd: u64,
        level: u64,
        optname: u64,
        optval: u64,
        optlen: u64,
    ) -> SysResult<u64>;
    fn getsockopt_value(&mut self, fd: u64, level: u64, optname: u64) -> SysResult<Vec<u8>>;
    fn connect(&mut self, fd: u64, address: u64, address_len: usize) -> SysResult<u64>;
    fn connect_blocking(&mut self, fd: u64, address: u64, address_len: usize)
    -> SyscallDisposition;
    fn accept(&mut self, fd: u64, address: u64, address_len: u64) -> SysResult<u64>;
    fn accept_blocking(&mut self, fd: u64, address: u64, address_len: u64) -> SyscallDisposition;
    fn accept4(&mut self, fd: u64, address: u64, address_len: u64, flags: u64) -> SysResult<u64>;
    fn accept4_blocking(
        &mut self,
        fd: u64,
        address: u64,
        address_len: u64,
        flags: u64,
    ) -> SyscallDisposition;
    fn sendto(
        &mut self,
        fd: u64,
        buffer: u64,
        len: usize,
        flags: u64,
        address: u64,
        address_len: usize,
    ) -> SysResult<u64>;
    fn sendto_blocking(
        &mut self,
        fd: u64,
        buffer: u64,
        len: usize,
        flags: u64,
        address: u64,
        address_len: usize,
    ) -> SyscallDisposition;
    fn recvfrom(
        &mut self,
        fd: u64,
        buffer: u64,
        len: usize,
        flags: u64,
        address: u64,
        address_len: u64,
    ) -> SysResult<u64>;
    fn recvfrom_blocking(
        &mut self,
        fd: u64,
        buffer: u64,
        len: usize,
        flags: u64,
        address: u64,
        address_len: u64,
    ) -> SyscallDisposition;
    fn sendmsg(&mut self, fd: u64, message: u64, flags: u64) -> SysResult<u64>;
    fn recvmsg(&mut self, fd: u64, message: u64, flags: u64) -> SysResult<u64>;
    fn sendmsg_blocking(&mut self, fd: u64, message: u64, flags: u64) -> SyscallDisposition;
    fn recvmsg_blocking(&mut self, fd: u64, message: u64, flags: u64) -> SyscallDisposition;
    fn shutdown(&mut self, fd: u64, how: u64) -> SysResult<u64>;
    fn bind(&mut self, fd: u64, address: u64, address_len: usize) -> SysResult<u64>;
    fn listen(&mut self, fd: u64, backlog: i32) -> SysResult<u64>;
    fn getsockname(&mut self, fd: u64, address: u64, address_len: u64) -> SysResult<u64>;
    fn getpeername(&mut self, fd: u64, address: u64, address_len: u64) -> SysResult<u64>;
    fn socketpair(
        &mut self,
        domain: i32,
        socket_type: u64,
        protocol: i32,
        sv: u64,
    ) -> SysResult<u64>;
    fn newfstatat(&mut self, dirfd: i64, path: &str, address: u64, flags: u64) -> SysResult<u64>;
    fn statx(
        &mut self,
        dirfd: i64,
        path: &str,
        flags: u64,
        mask: u64,
        address: u64,
    ) -> SysResult<u64>;
    fn getdents64(&mut self, fd: u64, address: u64, len: usize) -> SysResult<u64>;
    fn pipe(&mut self, pipefd: u64, flags: u64) -> SysResult<u64>;
    fn memfd_create(&mut self, name: &str, flags: u64) -> SysResult<u64>;
    fn eventfd(&mut self, initval: u32) -> SysResult<u64>;
    fn eventfd2(&mut self, initval: u32, flags: u64) -> SysResult<u64>;
    fn timerfd_create(&mut self, clockid: i32, flags: u64) -> SysResult<u64>;
    fn timerfd_settime(
        &mut self,
        fd: i32,
        flags: u64,
        new_value: u64,
        old_value: u64,
    ) -> SysResult<u64>;
    fn timerfd_gettime(&mut self, fd: i32, curr_value: u64) -> SysResult<u64>;
    fn signalfd(&mut self, fd: i32, mask: u64, sigsetsize: usize) -> SysResult<u64>;
    fn signalfd4(&mut self, fd: i32, mask: u64, sigsetsize: usize, flags: u64) -> SysResult<u64>;
    fn inotify_init(&mut self) -> SysResult<u64>;
    fn inotify_init1(&mut self, flags: u64) -> SysResult<u64>;
    fn inotify_add_watch(&mut self, fd: u64, path: &str, mask: u64) -> SysResult<u64>;
    fn inotify_rm_watch(&mut self, fd: u64, wd: i32) -> SysResult<u64>;
    fn uname(&mut self, address: u64) -> SysResult<u64>;
    fn set_robust_list(&mut self, head: u64, len: u64) -> SysResult<u64>;
    fn rseq(&mut self, area: u64, len: u64, flags: u64, signature: u64) -> SysResult<u64>;
    fn getrandom(&mut self, address: u64, len: usize, flags: u64) -> SysResult<u64>;
    fn rt_sigaction(
        &mut self,
        signal: u64,
        act: u64,
        oldact: u64,
        sigsetsize: u64,
    ) -> SysResult<u64>;
    fn futex(
        &mut self,
        uaddr: u64,
        operation: u64,
        val: u64,
        timeout: u64,
        uaddr2: u64,
        val3: u64,
    ) -> SysResult<u64>;
    fn futex_blocking(
        &mut self,
        uaddr: u64,
        operation: u64,
        val: u64,
        timeout: u64,
        uaddr2: u64,
        val3: u64,
    ) -> SyscallDisposition;
    fn geteuid(&self) -> SysResult<u64>;
    fn getegid(&self) -> SysResult<u64>;
    fn getuid(&self) -> SysResult<u64>;
    fn getgid(&self) -> SysResult<u64>;
    fn getresuid(&mut self, ruid: u64, euid: u64, suid: u64) -> SysResult<u64>;
    fn getresgid(&mut self, rgid: u64, egid: u64, sgid: u64) -> SysResult<u64>;
    fn getppid(&self) -> SysResult<u64>;
    fn gettid(&self) -> SysResult<u64>;
    fn setuid(&mut self, uid: u64) -> SysResult<u64>;
    fn setgid(&mut self, gid: u64) -> SysResult<u64>;
    fn setresuid(&mut self, ruid: u64, euid: u64, suid: u64) -> SysResult<u64>;
    fn setresgid(&mut self, rgid: u64, egid: u64, sgid: u64) -> SysResult<u64>;
    fn setgroups(&mut self, size: usize, list: u64) -> SysResult<u64>;
    fn getcwd(&mut self, address: u64, len: usize) -> SysResult<u64>;
    fn chdir(&mut self, path: &str) -> SysResult<u64>;
    fn fchdir(&mut self, fd: u64) -> SysResult<u64>;
    fn chmod(&mut self, path: &str, mode: u64) -> SysResult<u64>;
    fn fchmod(&mut self, fd: u64, mode: u64) -> SysResult<u64>;
    fn fchmodat(&mut self, dirfd: i64, path: &str, mode: u64) -> SysResult<u64>;
    fn chown(&mut self, path: &str, owner: u64, group: u64) -> SysResult<u64>;
    fn lchown(&mut self, path: &str, owner: u64, group: u64) -> SysResult<u64>;
    fn fchown(&mut self, fd: u64, owner: u64, group: u64) -> SysResult<u64>;
    fn fchownat(
        &mut self,
        dirfd: i64,
        path: &str,
        owner: u64,
        group: u64,
        flags: u64,
    ) -> SysResult<u64>;
    fn chroot(&mut self, path: &str) -> SysResult<u64>;
    fn mkdir(&mut self, path: &str, mode: u64) -> SysResult<u64>;
    fn stat_path(&mut self, path: &str, address: u64) -> SysResult<u64>;
    fn lstat_path(&mut self, path: &str, address: u64) -> SysResult<u64>;
    fn statfs_path(&mut self, path: &str, address: u64) -> SysResult<u64>;
    fn umask(&mut self, mask: u64) -> SysResult<u64>;
    fn rt_sigprocmask(
        &mut self,
        how: u64,
        set: u64,
        oldset: u64,
        sigsetsize: u64,
    ) -> SysResult<u64>;
    fn rt_sigsuspend(&mut self, mask: u64, sigsetsize: u64) -> SysResult<u64>;
    fn rt_sigsuspend_blocking(&mut self, mask: u64, sigsetsize: u64) -> SyscallDisposition;
    fn rt_sigreturn(&mut self) -> SysResult<u64>;
    fn fork(&mut self, flags: u64) -> SysResult<u64>;
    fn vfork_blocking(&mut self) -> SyscallDisposition;
    fn clone_process(&mut self, params: CloneParams) -> SysResult<u64>;
    fn clone3(&mut self, args: u64, size: usize) -> SysResult<u64>;
    fn wait4(&mut self, pid: i32, status: u64, options: u64, rusage: u64) -> SysResult<u64>;
    fn wait4_blocking(
        &mut self,
        pid: i32,
        status: u64,
        options: u64,
        rusage: u64,
    ) -> SyscallDisposition;
    fn send_signal(&mut self, pid: i32, signal: u64) -> SysResult<u64>;
    fn set_tid_address(&mut self, address: u64) -> SysResult<u64>;
    fn mount(
        &mut self,
        source: Option<&str>,
        target: &str,
        fstype: Option<&str>,
        flags: u64,
    ) -> SysResult<u64>;
    fn umount(&mut self, target: &str, flags: u64) -> SysResult<u64>;
    fn pivot_root(&mut self, new_root: &str, put_old: &str) -> SysResult<u64>;
    fn execve(&mut self, path: &str, argv: Vec<String>, envp: Vec<String>) -> SysResult<u64>;
    fn prctl(&mut self, option: u64, arg2: u64, arg3: u64, arg4: u64, arg5: u64) -> SysResult<u64>;
    fn epoll_create(&mut self, flags: u64) -> SysResult<u64>;
    fn epoll_create1(&mut self, flags: u64) -> SysResult<u64>;
    fn epoll_ctl(&mut self, epfd: u64, op: i32, fd: u64, event: u64) -> SysResult<u64>;
    fn epoll_wait(
        &mut self,
        epfd: u64,
        events: u64,
        maxevents: usize,
        timeout: i32,
    ) -> SysResult<u64>;
    fn epoll_wait_blocking(
        &mut self,
        epfd: u64,
        events: u64,
        maxevents: usize,
        timeout: i32,
    ) -> SyscallDisposition;
    fn epoll_pwait(
        &mut self,
        epfd: u64,
        events: u64,
        maxevents: usize,
        timeout: i32,
        sigmask: u64,
    ) -> SysResult<u64>;
    fn epoll_pwait_blocking(
        &mut self,
        epfd: u64,
        events: u64,
        maxevents: usize,
        timeout: i32,
        sigmask: u64,
    ) -> SyscallDisposition;
    fn gettimeofday(&mut self, tv: u64, tz: u64) -> SysResult<u64>;
    fn time(&mut self, tloc: u64) -> SysResult<u64>;
    fn clock_gettime(&mut self, clock_id: u64, tp: u64) -> SysResult<u64>;
    fn clock_getres(&mut self, clock_id: u64, tp: u64) -> SysResult<u64>;
    fn clock_nanosleep(
        &mut self,
        clock_id: u64,
        flags: u64,
        rqtp: u64,
        rmtp: u64,
    ) -> SysResult<u64>;
    fn clock_nanosleep_blocking(
        &mut self,
        clock_id: u64,
        flags: u64,
        rqtp: u64,
        rmtp: u64,
    ) -> SyscallDisposition;
    fn iopl(&mut self, level: u64) -> SysResult<u64>;
    fn arch_prctl(&mut self, code: u64, address: u64) -> SysResult<u64>;
    fn prlimit64(
        &mut self,
        pid: i32,
        resource: u64,
        new_limit: u64,
        old_limit: u64,
    ) -> SysResult<u64>;
    fn read_user_c_string(&self, address: u64, limit: usize) -> SysResult<String>;
    fn read_user_buffer(&self, address: u64, len: usize) -> SysResult<Vec<u8>>;
    fn read_user_pointer_array(&self, address: u64, limit: usize) -> SysResult<Vec<u64>>;
    fn write_user_buffer(&mut self, address: u64, bytes: &[u8]) -> SysResult<()>;
    fn write_user_timespec(&mut self, address: u64, secs: i64, nanos: i64) -> SysResult<()>;
    fn log_unimplemented(&mut self, number: u64, name: &str, args: SyscallArgs);
    fn log_unimplemented_command(
        &mut self,
        name: &str,
        command_name: &str,
        command: u64,
        args: SyscallArgs,
    );
}

#[macro_export]
macro_rules! declare_syscall {
    ($(#[$meta:meta])* $vis:vis struct $name:ident => $number:expr, $label:expr, |$ctx:ident, $args:ident| $body:block) => {
        $(#[$meta])*
        $vis struct $name;

        impl $name {
            fn handle(
                context: &mut dyn $crate::syscall::KernelSyscallContext,
                args: $crate::syscall::SyscallArgs,
            ) -> $crate::syscall::SyscallDisposition {
                let $ctx = context;
                let $args = args;
                $body
            }

            $vis const ENTRY: $crate::syscall::SyscallEntry = $crate::syscall::SyscallEntry {
                number: $number,
                name: $label,
                handle: Self::handle,
            };
        }
    };
}

#[macro_export]
macro_rules! register_syscalls {
    ($registry:expr, [$($handler:path),* $(,)?]) => {{
        $( $registry.register(<$handler>::ENTRY); )*
    }};
}

pub fn init() {
    handlers::init();
}

pub fn dispatch(
    number: u64,
    context: &mut dyn KernelSyscallContext,
    args: SyscallArgs,
) -> SyscallDispatch {
    registry::dispatch(number, context, args).unwrap_or_else(|| {
        context.log_unimplemented(number, "unknown", args);
        SyscallDispatch {
            disposition: SyscallDisposition::Return(Err(SysErr::NoSys)),
            name: "unknown",
        }
    })
}
