pub(crate) mod fd;
mod process;
mod user;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use aether_vfs::NodeRef;
use aether_vfs::{NodeKind, PollEvents};

use super::{KernelProcess, ProcessServices};
use crate::errno::SysErr;
use crate::errno::SysResult;
use crate::fs::{FileSystemIdentity, linux_open_flags};
use crate::process::FutexKey;
use crate::rootfs::FsLocation;
use crate::syscall::{
    BlockResult, BlockType, KernelSyscallContext, SyscallArgs, SyscallDisposition,
};

pub(crate) struct ProcessSyscallContext<'a, S> {
    pub(crate) process: &'a mut KernelProcess,
    pub(crate) services: S,
}

impl<S> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_pid(&self) -> u32 {
        self.process.identity.pid
    }

    pub(crate) fn masked_mode(&self, mode: u64, file_type: u32) -> u32 {
        file_type | ((mode as u32) & !u32::from(self.process.umask) & 0o777)
    }

    pub(crate) fn open_node(
        &mut self,
        node: NodeRef,
        filesystem: FileSystemIdentity,
        location: Option<FsLocation>,
        flags: u64,
    ) -> SysResult<u64> {
        const O_CLOEXEC: u64 = 0o2000000;
        const O_DIRECTORY: u64 = 0o200000;
        const O_TRUNC: u64 = 0o1000;

        if (flags & O_DIRECTORY) != 0 && node.kind() != NodeKind::Directory {
            return Err(SysErr::NotDir);
        }

        if (flags & O_TRUNC) != 0 && node.kind() == NodeKind::File {
            node.truncate(0).map_err(SysErr::from)?;
        }

        Ok(self.process.files.insert_node(
            node,
            linux_open_flags(flags),
            filesystem,
            location,
            (flags & O_CLOEXEC) != 0,
        ) as u64)
    }

    pub(crate) fn dup_to(
        &mut self,
        oldfd: u64,
        newfd: u64,
        flags: u64,
        reject_same_fd: bool,
    ) -> SysResult<u64> {
        const O_CLOEXEC: u64 = 0o2000000;

        if oldfd == newfd {
            return if reject_same_fd || flags != 0 {
                Err(SysErr::Inval)
            } else {
                self.process
                    .files
                    .get(oldfd as u32)
                    .map(|_| newfd)
                    .ok_or(SysErr::BadFd)
            };
        }

        if flags & !O_CLOEXEC != 0 {
            return Err(SysErr::Inval);
        }

        self.process
            .files
            .duplicate_to(oldfd as u32, newfd as u32, (flags & O_CLOEXEC) != 0)
            .map(u64::from)
            .ok_or(SysErr::BadFd)
    }

    fn block_current_syscall(&mut self, block: BlockType) -> SyscallDisposition {
        SyscallDisposition::block(block)
    }

    fn take_wake_result_or_block(
        &mut self,
        block: BlockType,
    ) -> Result<BlockResult, SyscallDisposition> {
        if let Some(result) = self.process.wake_result.take() {
            return Ok(result);
        }
        Err(self.block_current_syscall(block))
    }

    pub(crate) fn wait_file(
        &mut self,
        fd: u32,
        events: PollEvents,
    ) -> Result<BlockResult, SyscallDisposition> {
        self.take_wake_result_or_block(BlockType::File { fd, events })
    }

    pub(crate) fn wait_poll(
        &mut self,
        deadline_nanos: Option<u64>,
        registrations: &[crate::process::PendingPollRegistration],
    ) -> Result<BlockResult, SyscallDisposition> {
        if let Some(result) = self.process.wake_result.take() {
            self.process.pending_file_waits.clear();
            return Ok(result);
        }
        self.process.pending_file_waits.clear();
        self.process
            .pending_file_waits
            .extend_from_slice(registrations);
        Err(self.block_current_syscall(BlockType::Poll { deadline_nanos }))
    }

    pub(crate) fn wait_timer(
        &mut self,
        target_nanos: u64,
        request_nanos: u64,
        rmtp: u64,
        flags: u64,
    ) -> Result<BlockResult, SyscallDisposition> {
        self.take_wake_result_or_block(BlockType::Timer {
            target_nanos,
            request_nanos,
            rmtp,
            flags,
        })
    }

    pub(crate) fn wait_futex(
        &mut self,
        key: FutexKey,
        bitset: u32,
        deadline_nanos: Option<u64>,
    ) -> Result<BlockResult, SyscallDisposition> {
        self.take_wake_result_or_block(BlockType::Futex {
            key,
            bitset,
            deadline_nanos,
        })
    }

    pub(crate) fn wait_wait_child(
        &mut self,
        pid: i32,
        status_ptr: u64,
        options: u64,
    ) -> Result<BlockResult, SyscallDisposition> {
        self.take_wake_result_or_block(BlockType::WaitChild {
            pid,
            status_ptr,
            options,
        })
    }

    pub(crate) fn wait_vfork(&mut self, child: u32) -> Result<BlockResult, SyscallDisposition> {
        self.take_wake_result_or_block(BlockType::Vfork { child })
    }

    pub(crate) fn wait_signal_suspend(&mut self) -> Result<BlockResult, SyscallDisposition> {
        self.take_wake_result_or_block(BlockType::SignalSuspend)
    }

    pub(crate) fn file_blocking_syscall<F>(
        &mut self,
        fd: u32,
        events: PollEvents,
        mut syscall: F,
    ) -> SyscallDisposition
    where
        F: FnMut(&mut Self) -> SysResult<u64>,
    {
        let nonblock = self
            .process
            .files
            .get(fd)
            .map(|descriptor| descriptor.file.lock().flags().nonblock())
            .unwrap_or(false);
        if nonblock {
            return match syscall(self) {
                Ok(value) => SyscallDisposition::ok(value),
                Err(error) => SyscallDisposition::err(error),
            };
        }

        loop {
            match syscall(self) {
                Ok(value) => return SyscallDisposition::ok(value),
                Err(SysErr::Again) => match self.wait_file(fd, events) {
                    Ok(BlockResult::File { ready: true }) => {}
                    Ok(BlockResult::SignalInterrupted) => {
                        return SyscallDisposition::err(SysErr::Intr);
                    }
                    Ok(_) => return SyscallDisposition::err(SysErr::Intr),
                    Err(disposition) => return disposition,
                },
                Err(error) => return SyscallDisposition::err(error),
            }
        }
    }
}

impl<S: ProcessServices> KernelSyscallContext for ProcessSyscallContext<'_, S> {
    fn pid(&self) -> u32 {
        Self::syscall_pid(self)
    }
    fn take_wake_result(&mut self) -> Option<crate::syscall::BlockResult> {
        self.process.wake_result.take()
    }
    fn has_wake_result(&self) -> bool {
        self.process.wake_result.is_some()
    }
    fn arch_prctl(&mut self, code: u64, address: u64) -> SysResult<u64> {
        Self::syscall_arch_prctl(self, code, address)
    }
    fn brk(&mut self, address: u64) -> SysResult<u64> {
        Self::syscall_brk(self, address)
    }
    fn mmap(
        &mut self,
        address: u64,
        len: u64,
        prot: u64,
        flags: u64,
        fd: u64,
        offset: u64,
    ) -> SysResult<u64> {
        Self::syscall_mmap(self, address, len, prot, flags, fd, offset)
    }
    fn access(&mut self, path: &str, mode: u64) -> SysResult<u64> {
        Self::syscall_access(self, path, mode)
    }
    fn faccessat(&mut self, dirfd: i64, path: &str, mode: u64, flags: u64) -> SysResult<u64> {
        Self::syscall_faccessat(self, dirfd, path, mode, flags)
    }
    fn munmap(&mut self, address: u64, len: u64) -> SysResult<u64> {
        Self::syscall_munmap(self, address, len)
    }
    fn mprotect(&mut self, address: u64, len: u64, prot: u64) -> SysResult<u64> {
        Self::syscall_mprotect(self, address, len, prot)
    }
    fn mremap(
        &mut self,
        old_address: u64,
        old_size: u64,
        new_size: u64,
        flags: u64,
        new_address: u64,
    ) -> SysResult<u64> {
        Self::syscall_mremap(self, old_address, old_size, new_size, flags, new_address)
    }
    fn openat(&mut self, dirfd: i64, path: &str, flags: u64, mode: u64) -> SysResult<u64> {
        Self::syscall_openat(self, dirfd, path, flags, mode)
    }
    fn creat(&mut self, path: &str, mode: u64) -> SysResult<u64> {
        Self::syscall_creat(self, path, mode)
    }
    fn link(&mut self, old_path: &str, new_path: &str) -> SysResult<u64> {
        Self::syscall_link(self, old_path, new_path)
    }
    fn linkat(
        &mut self,
        olddirfd: i64,
        old_path: &str,
        newdirfd: i64,
        new_path: &str,
        flags: u64,
    ) -> SysResult<u64> {
        Self::syscall_linkat(self, olddirfd, old_path, newdirfd, new_path, flags)
    }
    fn symlink(&mut self, target: &str, linkpath: &str) -> SysResult<u64> {
        Self::syscall_symlink(self, target, linkpath)
    }
    fn unlinkat(&mut self, dirfd: i64, path: &str, flags: u64) -> SysResult<u64> {
        Self::syscall_unlinkat(self, dirfd, path, flags)
    }
    fn readlinkat(&mut self, dirfd: i64, path: &str, address: u64, len: usize) -> SysResult<u64> {
        Self::syscall_readlinkat(self, dirfd, path, address, len)
    }
    fn rename(&mut self, old_path: &str, new_path: &str) -> SysResult<u64> {
        Self::syscall_rename(self, old_path, new_path)
    }
    fn renameat(
        &mut self,
        olddirfd: i64,
        old_path: &str,
        newdirfd: i64,
        new_path: &str,
    ) -> SysResult<u64> {
        Self::syscall_renameat(self, olddirfd, old_path, newdirfd, new_path)
    }
    fn close_fd(&mut self, fd: u64) -> SysResult<u64> {
        Self::syscall_close_fd(self, fd)
    }
    fn close_range(&mut self, first: u64, last: u64, flags: u64) -> SysResult<u64> {
        Self::syscall_close_range(self, first, last, flags)
    }
    fn lseek(&mut self, fd: u64, offset: i64, whence: u64) -> SysResult<u64> {
        Self::syscall_lseek(self, fd, offset, whence)
    }
    fn read_fd(&mut self, fd: u64, address: u64, len: usize) -> SysResult<u64> {
        Self::syscall_read_fd(self, fd, address, len)
    }
    fn read_fd_blocking(
        &mut self,
        fd: u64,
        address: u64,
        len: usize,
    ) -> crate::syscall::SyscallDisposition {
        Self::syscall_read_fd_blocking(self, fd, address, len)
    }
    fn write_fd(&mut self, fd: u64, address: u64, len: usize) -> SysResult<u64> {
        Self::syscall_write_fd(self, fd, address, len)
    }
    fn write_fd_blocking(
        &mut self,
        fd: u64,
        address: u64,
        len: usize,
    ) -> crate::syscall::SyscallDisposition {
        Self::syscall_write_fd_blocking(self, fd, address, len)
    }
    fn poll(&mut self, fds: u64, nfds: usize, timeout: i32) -> SysResult<u64> {
        Self::syscall_poll(self, fds, nfds, timeout)
    }
    fn poll_blocking(
        &mut self,
        fds: u64,
        nfds: usize,
        timeout: i32,
    ) -> crate::syscall::SyscallDisposition {
        Self::syscall_poll_blocking(self, fds, nfds, timeout)
    }
    fn ppoll(
        &mut self,
        fds: u64,
        nfds: usize,
        timeout: u64,
        sigmask: u64,
        sigsetsize: usize,
    ) -> SysResult<u64> {
        Self::syscall_ppoll(self, fds, nfds, timeout, sigmask, sigsetsize)
    }
    fn ppoll_blocking(
        &mut self,
        fds: u64,
        nfds: usize,
        timeout: u64,
        sigmask: u64,
        sigsetsize: usize,
    ) -> crate::syscall::SyscallDisposition {
        Self::syscall_ppoll_blocking(self, fds, nfds, timeout, sigmask, sigsetsize)
    }
    fn sendfile(&mut self, out_fd: u64, in_fd: u64, offset: u64, count: usize) -> SysResult<u64> {
        Self::syscall_sendfile(self, out_fd, in_fd, offset, count)
    }
    fn sendfile_blocking(
        &mut self,
        out_fd: u64,
        in_fd: u64,
        offset: u64,
        count: usize,
    ) -> crate::syscall::SyscallDisposition {
        Self::syscall_sendfile_blocking(self, out_fd, in_fd, offset, count)
    }
    fn pread64(&mut self, fd: u64, address: u64, len: usize, offset: u64) -> SysResult<u64> {
        Self::syscall_pread64(self, fd, address, len, offset)
    }
    fn pwrite64(&mut self, fd: u64, address: u64, len: usize, offset: u64) -> SysResult<u64> {
        Self::syscall_pwrite64(self, fd, address, len, offset)
    }
    fn readv_fd(&mut self, fd: u64, iov: u64, iovcnt: usize) -> SysResult<u64> {
        Self::syscall_readv_fd(self, fd, iov, iovcnt)
    }
    fn readv_fd_blocking(
        &mut self,
        fd: u64,
        iov: u64,
        iovcnt: usize,
    ) -> crate::syscall::SyscallDisposition {
        Self::syscall_readv_fd_blocking(self, fd, iov, iovcnt)
    }
    fn writev_fd(&mut self, fd: u64, iov: u64, iovcnt: usize) -> SysResult<u64> {
        Self::syscall_writev_fd(self, fd, iov, iovcnt)
    }
    fn writev_fd_blocking(
        &mut self,
        fd: u64,
        iov: u64,
        iovcnt: usize,
    ) -> crate::syscall::SyscallDisposition {
        Self::syscall_writev_fd_blocking(self, fd, iov, iovcnt)
    }
    fn preadv64(&mut self, fd: u64, iov: u64, iovcnt: usize, offset: u64) -> SysResult<u64> {
        Self::syscall_preadv64(self, fd, iov, iovcnt, offset)
    }
    fn pwritev64(&mut self, fd: u64, iov: u64, iovcnt: usize, offset: u64) -> SysResult<u64> {
        Self::syscall_pwritev64(self, fd, iov, iovcnt, offset)
    }
    fn fadvise64(&mut self, fd: u64, offset: u64, len: u64, advice: u64) -> SysResult<u64> {
        Self::syscall_fadvise64(self, fd, offset, len, advice)
    }
    fn fallocate(&mut self, fd: u64, mode: u64, offset: i64, len: i64) -> SysResult<u64> {
        Self::syscall_fallocate(self, fd, mode, offset, len)
    }
    fn ioctl_fd(&mut self, fd: u64, command: u64, argument: u64) -> SysResult<u64> {
        Self::syscall_ioctl_fd(self, fd, command, argument)
    }
    fn flock(&mut self, fd: u64, operation: u64) -> SysResult<u64> {
        Self::syscall_flock(self, fd, operation)
    }
    fn flock_blocking(&mut self, fd: u64, operation: u64) -> SyscallDisposition {
        Self::syscall_flock_blocking(self, fd, operation)
    }
    fn fcntl(&mut self, fd: u64, command: u64, arg: u64) -> SysResult<u64> {
        Self::syscall_fcntl(self, fd, command, arg)
    }
    fn fstat(&mut self, fd: u64, address: u64) -> SysResult<u64> {
        Self::syscall_fstat(self, fd, address)
    }
    fn fstatfs(&mut self, fd: u64, address: u64) -> SysResult<u64> {
        Self::syscall_fstatfs(self, fd, address)
    }
    fn dup(&mut self, fd: u64) -> SysResult<u64> {
        Self::syscall_dup(self, fd)
    }
    fn dup2(&mut self, oldfd: u64, newfd: u64) -> SysResult<u64> {
        Self::syscall_dup2(self, oldfd, newfd)
    }
    fn dup3(&mut self, oldfd: u64, newfd: u64, flags: u64) -> SysResult<u64> {
        Self::syscall_dup3(self, oldfd, newfd, flags)
    }
    fn socket(&mut self, domain: i32, socket_type: u64, protocol: i32) -> SysResult<u64> {
        Self::syscall_socket(self, domain, socket_type, protocol)
    }
    fn setsockopt(
        &mut self,
        fd: u64,
        level: u64,
        optname: u64,
        optval: u64,
        optlen: u64,
    ) -> SysResult<u64> {
        Self::syscall_setsockopt(self, fd, level, optname, optval, optlen)
    }
    fn getsockopt_value(&mut self, fd: u64, level: u64, optname: u64) -> SysResult<Vec<u8>> {
        Self::syscall_getsockopt_value(self, fd, level, optname)
    }
    fn connect(&mut self, fd: u64, address: u64, address_len: usize) -> SysResult<u64> {
        Self::syscall_connect(self, fd, address, address_len)
    }
    fn connect_blocking(
        &mut self,
        fd: u64,
        address: u64,
        address_len: usize,
    ) -> crate::syscall::SyscallDisposition {
        Self::syscall_connect_blocking(self, fd, address, address_len)
    }
    fn accept(&mut self, fd: u64, address: u64, address_len: u64) -> SysResult<u64> {
        Self::syscall_accept(self, fd, address, address_len)
    }
    fn accept_blocking(
        &mut self,
        fd: u64,
        address: u64,
        address_len: u64,
    ) -> crate::syscall::SyscallDisposition {
        Self::syscall_accept_blocking(self, fd, address, address_len)
    }
    fn accept4(&mut self, fd: u64, address: u64, address_len: u64, flags: u64) -> SysResult<u64> {
        Self::syscall_accept4(self, fd, address, address_len, flags)
    }
    fn accept4_blocking(
        &mut self,
        fd: u64,
        address: u64,
        address_len: u64,
        flags: u64,
    ) -> crate::syscall::SyscallDisposition {
        Self::syscall_accept4_blocking(self, fd, address, address_len, flags)
    }
    fn sendto(
        &mut self,
        fd: u64,
        buffer: u64,
        len: usize,
        flags: u64,
        address: u64,
        address_len: usize,
    ) -> SysResult<u64> {
        Self::syscall_sendto(self, fd, buffer, len, flags, address, address_len)
    }
    fn sendto_blocking(
        &mut self,
        fd: u64,
        buffer: u64,
        len: usize,
        flags: u64,
        address: u64,
        address_len: usize,
    ) -> crate::syscall::SyscallDisposition {
        Self::syscall_sendto_blocking(self, fd, buffer, len, flags, address, address_len)
    }
    fn recvfrom(
        &mut self,
        fd: u64,
        buffer: u64,
        len: usize,
        flags: u64,
        address: u64,
        address_len: u64,
    ) -> SysResult<u64> {
        Self::syscall_recvfrom(self, fd, buffer, len, flags, address, address_len)
    }
    fn recvfrom_blocking(
        &mut self,
        fd: u64,
        buffer: u64,
        len: usize,
        flags: u64,
        address: u64,
        address_len: u64,
    ) -> crate::syscall::SyscallDisposition {
        Self::syscall_recvfrom_blocking(self, fd, buffer, len, flags, address, address_len)
    }
    fn sendmsg(&mut self, fd: u64, message: u64, flags: u64) -> SysResult<u64> {
        Self::syscall_sendmsg(self, fd, message, flags)
    }
    fn recvmsg(&mut self, fd: u64, message: u64, flags: u64) -> SysResult<u64> {
        Self::syscall_recvmsg(self, fd, message, flags)
    }
    fn sendmsg_blocking(
        &mut self,
        fd: u64,
        message: u64,
        flags: u64,
    ) -> crate::syscall::SyscallDisposition {
        Self::syscall_sendmsg_blocking(self, fd, message, flags)
    }
    fn recvmsg_blocking(
        &mut self,
        fd: u64,
        message: u64,
        flags: u64,
    ) -> crate::syscall::SyscallDisposition {
        Self::syscall_recvmsg_blocking(self, fd, message, flags)
    }
    fn shutdown(&mut self, fd: u64, how: u64) -> SysResult<u64> {
        Self::syscall_shutdown(self, fd, how)
    }
    fn bind(&mut self, fd: u64, address: u64, address_len: usize) -> SysResult<u64> {
        Self::syscall_bind(self, fd, address, address_len)
    }
    fn listen(&mut self, fd: u64, backlog: i32) -> SysResult<u64> {
        Self::syscall_listen(self, fd, backlog)
    }
    fn getsockname(&mut self, fd: u64, address: u64, address_len: u64) -> SysResult<u64> {
        Self::syscall_getsockname(self, fd, address, address_len)
    }
    fn getpeername(&mut self, fd: u64, address: u64, address_len: u64) -> SysResult<u64> {
        Self::syscall_getpeername(self, fd, address, address_len)
    }
    fn socketpair(
        &mut self,
        domain: i32,
        socket_type: u64,
        protocol: i32,
        sv: u64,
    ) -> SysResult<u64> {
        Self::syscall_socketpair(self, domain, socket_type, protocol, sv)
    }
    fn newfstatat(&mut self, dirfd: i64, path: &str, address: u64, flags: u64) -> SysResult<u64> {
        Self::syscall_newfstatat(self, dirfd, path, address, flags)
    }
    fn statx(
        &mut self,
        dirfd: i64,
        path: &str,
        flags: u64,
        mask: u64,
        address: u64,
    ) -> SysResult<u64> {
        Self::syscall_statx(self, dirfd, path, flags, mask, address)
    }
    fn getdents64(&mut self, fd: u64, address: u64, len: usize) -> SysResult<u64> {
        Self::syscall_getdents64(self, fd, address, len)
    }
    fn pipe(&mut self, pipefd: u64, flags: u64) -> SysResult<u64> {
        Self::syscall_pipe(self, pipefd, flags)
    }
    fn memfd_create(&mut self, name: &str, flags: u64) -> SysResult<u64> {
        Self::syscall_memfd_create(self, name, flags)
    }
    fn eventfd(&mut self, initval: u32) -> SysResult<u64> {
        Self::syscall_eventfd(self, initval)
    }
    fn eventfd2(&mut self, initval: u32, flags: u64) -> SysResult<u64> {
        Self::syscall_eventfd2(self, initval, flags)
    }
    fn timerfd_create(&mut self, clockid: i32, flags: u64) -> SysResult<u64> {
        Self::syscall_timerfd_create(self, clockid, flags)
    }
    fn timerfd_settime(
        &mut self,
        fd: i32,
        flags: u64,
        new_value: u64,
        old_value: u64,
    ) -> SysResult<u64> {
        Self::syscall_timerfd_settime(self, fd, flags, new_value, old_value)
    }
    fn timerfd_gettime(&mut self, fd: i32, curr_value: u64) -> SysResult<u64> {
        Self::syscall_timerfd_gettime(self, fd, curr_value)
    }
    fn signalfd(&mut self, fd: i32, mask: u64, sigsetsize: usize) -> SysResult<u64> {
        Self::syscall_signalfd4(self, fd, mask, sigsetsize, 0)
    }
    fn signalfd4(&mut self, fd: i32, mask: u64, sigsetsize: usize, flags: u64) -> SysResult<u64> {
        Self::syscall_signalfd4(self, fd, mask, sigsetsize, flags)
    }
    fn inotify_init(&mut self) -> SysResult<u64> {
        Self::syscall_inotify_init(self)
    }
    fn inotify_init1(&mut self, flags: u64) -> SysResult<u64> {
        Self::syscall_inotify_init1(self, flags)
    }
    fn inotify_add_watch(&mut self, fd: u64, path: &str, mask: u64) -> SysResult<u64> {
        Self::syscall_inotify_add_watch(self, fd, path, mask)
    }
    fn inotify_rm_watch(&mut self, fd: u64, wd: i32) -> SysResult<u64> {
        Self::syscall_inotify_rm_watch(self, fd, wd)
    }
    fn uname(&mut self, address: u64) -> SysResult<u64> {
        Self::syscall_uname(self, address)
    }
    fn set_robust_list(&mut self, head: u64, len: u64) -> SysResult<u64> {
        Self::syscall_set_robust_list(self, head, len)
    }
    fn rseq(&mut self, area: u64, len: u64, flags: u64, signature: u64) -> SysResult<u64> {
        Self::syscall_rseq(self, area, len, flags, signature)
    }
    fn getrandom(&mut self, address: u64, len: usize, flags: u64) -> SysResult<u64> {
        Self::syscall_getrandom(self, address, len, flags)
    }
    fn rt_sigaction(
        &mut self,
        signal: u64,
        act: u64,
        oldact: u64,
        sigsetsize: u64,
    ) -> SysResult<u64> {
        Self::syscall_rt_sigaction(self, signal, act, oldact, sigsetsize)
    }
    fn futex(
        &mut self,
        uaddr: u64,
        operation: u64,
        val: u64,
        timeout: u64,
        uaddr2: u64,
        val3: u64,
    ) -> SysResult<u64> {
        Self::syscall_futex(self, uaddr, operation, val, timeout, uaddr2, val3)
    }
    fn futex_blocking(
        &mut self,
        uaddr: u64,
        operation: u64,
        val: u64,
        timeout: u64,
        uaddr2: u64,
        val3: u64,
    ) -> crate::syscall::SyscallDisposition {
        Self::syscall_futex_blocking(self, uaddr, operation, val, timeout, uaddr2, val3)
    }
    fn geteuid(&self) -> SysResult<u64> {
        Self::syscall_geteuid(self)
    }
    fn getegid(&self) -> SysResult<u64> {
        Self::syscall_getegid(self)
    }
    fn getuid(&self) -> SysResult<u64> {
        Self::syscall_getuid(self)
    }
    fn getgid(&self) -> SysResult<u64> {
        Self::syscall_getgid(self)
    }
    fn getpgid(&self) -> SysResult<u64> {
        Self::syscall_getpgid(self)
    }
    fn getresuid(&mut self, ruid: u64, euid: u64, suid: u64) -> SysResult<u64> {
        Self::syscall_getresuid(self, ruid, euid, suid)
    }
    fn getresgid(&mut self, rgid: u64, egid: u64, sgid: u64) -> SysResult<u64> {
        Self::syscall_getresgid(self, rgid, egid, sgid)
    }
    fn getppid(&self) -> SysResult<u64> {
        Self::syscall_getppid(self)
    }
    fn gettid(&self) -> SysResult<u64> {
        Self::syscall_gettid(self)
    }
    fn setuid(&mut self, uid: u64) -> SysResult<u64> {
        Self::syscall_setuid(self, uid)
    }
    fn setgid(&mut self, gid: u64) -> SysResult<u64> {
        Self::syscall_setgid(self, gid)
    }
    fn setresuid(&mut self, ruid: u64, euid: u64, suid: u64) -> SysResult<u64> {
        Self::syscall_setresuid(self, ruid, euid, suid)
    }
    fn setresgid(&mut self, rgid: u64, egid: u64, sgid: u64) -> SysResult<u64> {
        Self::syscall_setresgid(self, rgid, egid, sgid)
    }
    fn setgroups(&mut self, size: usize, list: u64) -> SysResult<u64> {
        Self::syscall_setgroups(self, size, list)
    }
    fn getcwd(&mut self, address: u64, len: usize) -> SysResult<u64> {
        Self::syscall_getcwd(self, address, len)
    }
    fn chdir(&mut self, path: &str) -> SysResult<u64> {
        Self::syscall_chdir(self, path)
    }
    fn fchdir(&mut self, fd: u64) -> SysResult<u64> {
        Self::syscall_fchdir(self, fd)
    }
    fn chmod(&mut self, path: &str, mode: u64) -> SysResult<u64> {
        Self::syscall_chmod(self, path, mode)
    }
    fn fchmod(&mut self, fd: u64, mode: u64) -> SysResult<u64> {
        Self::syscall_fchmod(self, fd, mode)
    }
    fn fchmodat(&mut self, dirfd: i64, path: &str, mode: u64) -> SysResult<u64> {
        Self::syscall_fchmodat(self, dirfd, path, mode)
    }
    fn chown(&mut self, path: &str, owner: u64, group: u64) -> SysResult<u64> {
        Self::syscall_chown(self, path, owner, group)
    }
    fn lchown(&mut self, path: &str, owner: u64, group: u64) -> SysResult<u64> {
        Self::syscall_lchown(self, path, owner, group)
    }
    fn fchown(&mut self, fd: u64, owner: u64, group: u64) -> SysResult<u64> {
        Self::syscall_fchown(self, fd, owner, group)
    }
    fn fchownat(
        &mut self,
        dirfd: i64,
        path: &str,
        owner: u64,
        group: u64,
        flags: u64,
    ) -> SysResult<u64> {
        Self::syscall_fchownat(self, dirfd, path, owner, group, flags)
    }
    fn chroot(&mut self, path: &str) -> SysResult<u64> {
        Self::syscall_chroot(self, path)
    }
    fn mkdir(&mut self, path: &str, mode: u64) -> SysResult<u64> {
        Self::syscall_mkdir(self, path, mode)
    }
    fn stat_path(&mut self, path: &str, address: u64) -> SysResult<u64> {
        Self::syscall_stat_path(self, path, address)
    }
    fn lstat_path(&mut self, path: &str, address: u64) -> SysResult<u64> {
        Self::syscall_lstat_path(self, path, address)
    }
    fn statfs_path(&mut self, path: &str, address: u64) -> SysResult<u64> {
        Self::syscall_statfs_path(self, path, address)
    }
    fn umask(&mut self, mask: u64) -> SysResult<u64> {
        Self::syscall_umask(self, mask)
    }
    fn rt_sigprocmask(
        &mut self,
        how: u64,
        set: u64,
        oldset: u64,
        sigsetsize: u64,
    ) -> SysResult<u64> {
        Self::syscall_rt_sigprocmask(self, how, set, oldset, sigsetsize)
    }
    fn rt_sigsuspend(&mut self, mask: u64, sigsetsize: u64) -> SysResult<u64> {
        Self::syscall_rt_sigsuspend(self, mask, sigsetsize)
    }
    fn rt_sigsuspend_blocking(
        &mut self,
        mask: u64,
        sigsetsize: u64,
    ) -> crate::syscall::SyscallDisposition {
        Self::syscall_rt_sigsuspend_blocking(self, mask, sigsetsize)
    }
    fn rt_sigreturn(&mut self) -> SysResult<u64> {
        Self::syscall_rt_sigreturn(self)
    }
    fn fork(&mut self, flags: u64) -> SysResult<u64> {
        Self::syscall_fork(self, flags)
    }
    fn vfork_blocking(&mut self) -> crate::syscall::SyscallDisposition {
        Self::syscall_vfork_blocking(self)
    }
    fn clone_process(&mut self, params: crate::process::CloneParams) -> SysResult<u64> {
        Self::syscall_clone_process(self, params)
    }
    fn clone3(&mut self, args: u64, size: usize) -> SysResult<u64> {
        Self::syscall_clone3(self, args, size)
    }
    fn wait4(&mut self, pid: i32, status: u64, options: u64, rusage: u64) -> SysResult<u64> {
        Self::syscall_wait4(self, pid, status, options, rusage)
    }
    fn wait4_blocking(
        &mut self,
        pid: i32,
        status: u64,
        options: u64,
        rusage: u64,
    ) -> crate::syscall::SyscallDisposition {
        Self::syscall_wait4_blocking(self, pid, status, options, rusage)
    }
    fn send_signal(&mut self, pid: i32, signal: u64) -> SysResult<u64> {
        Self::syscall_send_signal(self, pid, signal)
    }
    fn read_user_c_string(&self, address: u64, limit: usize) -> SysResult<String> {
        Self::syscall_read_user_c_string(self, address, limit)
    }
    fn read_user_buffer(&self, address: u64, len: usize) -> SysResult<Vec<u8>> {
        Self::syscall_read_user_buffer(self, address, len)
    }
    fn read_user_pointer_array(&self, address: u64, limit: usize) -> SysResult<Vec<u64>> {
        Self::syscall_read_user_pointer_array(self, address, limit)
    }
    fn write_user_buffer(&mut self, address: u64, bytes: &[u8]) -> SysResult<()> {
        Self::syscall_write_user_buffer(self, address, bytes)
    }
    fn write_user_timespec(&mut self, address: u64, secs: i64, nanos: i64) -> SysResult<()> {
        Self::syscall_write_user_timespec(self, address, secs, nanos)
    }
    fn set_tid_address(&mut self, address: u64) -> SysResult<u64> {
        Self::syscall_set_tid_address(self, address)
    }
    fn mount(
        &mut self,
        source: Option<&str>,
        target: &str,
        fstype: Option<&str>,
        flags: u64,
    ) -> SysResult<u64> {
        Self::syscall_mount(self, source, target, fstype, flags)
    }
    fn umount(&mut self, target: &str, flags: u64) -> SysResult<u64> {
        Self::syscall_umount(self, target, flags)
    }
    fn pivot_root(&mut self, new_root: &str, put_old: &str) -> SysResult<u64> {
        Self::syscall_pivot_root(self, new_root, put_old)
    }
    fn execve(&mut self, path: &str, argv: Vec<String>, envp: Vec<String>) -> SysResult<u64> {
        Self::syscall_execve(self, path, argv, envp)
    }
    fn prctl(&mut self, option: u64, arg2: u64, arg3: u64, arg4: u64, arg5: u64) -> SysResult<u64> {
        Self::syscall_prctl(self, option, arg2, arg3, arg4, arg5)
    }
    fn epoll_create(&mut self, flags: u64) -> SysResult<u64> {
        Self::syscall_epoll_create(self, flags)
    }
    fn epoll_create1(&mut self, flags: u64) -> SysResult<u64> {
        Self::syscall_epoll_create1(self, flags)
    }
    fn epoll_ctl(&mut self, epfd: u64, op: i32, fd: u64, event: u64) -> SysResult<u64> {
        Self::syscall_epoll_ctl(self, epfd, op, fd, event)
    }
    fn epoll_wait(
        &mut self,
        epfd: u64,
        events: u64,
        maxevents: usize,
        timeout: i32,
    ) -> SysResult<u64> {
        Self::syscall_epoll_wait(self, epfd, events, maxevents, timeout)
    }
    fn epoll_wait_blocking(
        &mut self,
        epfd: u64,
        events: u64,
        maxevents: usize,
        timeout: i32,
    ) -> crate::syscall::SyscallDisposition {
        Self::syscall_epoll_wait_blocking(self, epfd, events, maxevents, timeout)
    }
    fn epoll_pwait(
        &mut self,
        epfd: u64,
        events: u64,
        maxevents: usize,
        timeout: i32,
        sigmask: u64,
    ) -> SysResult<u64> {
        Self::syscall_epoll_pwait(self, epfd, events, maxevents, timeout, sigmask)
    }
    fn epoll_pwait_blocking(
        &mut self,
        epfd: u64,
        events: u64,
        maxevents: usize,
        timeout: i32,
        sigmask: u64,
    ) -> crate::syscall::SyscallDisposition {
        Self::syscall_epoll_pwait_blocking(self, epfd, events, maxevents, timeout, sigmask)
    }
    fn gettimeofday(&mut self, tv: u64, tz: u64) -> SysResult<u64> {
        Self::syscall_gettimeofday(self, tv, tz)
    }
    fn time(&mut self, tloc: u64) -> SysResult<u64> {
        Self::syscall_time(self, tloc)
    }
    fn clock_gettime(&mut self, clock_id: u64, tp: u64) -> SysResult<u64> {
        Self::syscall_clock_gettime(self, clock_id, tp)
    }
    fn clock_getres(&mut self, clock_id: u64, tp: u64) -> SysResult<u64> {
        Self::syscall_clock_getres(self, clock_id, tp)
    }
    fn clock_nanosleep(
        &mut self,
        clock_id: u64,
        flags: u64,
        rqtp: u64,
        rmtp: u64,
    ) -> SysResult<u64> {
        Self::syscall_clock_nanosleep(self, clock_id, flags, rqtp, rmtp)
    }
    fn clock_nanosleep_blocking(
        &mut self,
        clock_id: u64,
        flags: u64,
        rqtp: u64,
        rmtp: u64,
    ) -> crate::syscall::SyscallDisposition {
        Self::syscall_clock_nanosleep_blocking(self, clock_id, flags, rqtp, rmtp)
    }
    fn iopl(&mut self, _level: u64) -> SysResult<u64> {
        Err(SysErr::NoSys)
    }
    fn prlimit64(
        &mut self,
        pid: i32,
        resource: u64,
        new_limit: u64,
        old_limit: u64,
    ) -> SysResult<u64> {
        Self::syscall_prlimit64(self, pid, resource, new_limit, old_limit)
    }
    fn log_unimplemented(&mut self, number: u64, name: &str, args: SyscallArgs) {
        Self::syscall_log_unimplemented(self, number, name, args)
    }
    fn log_unimplemented_command(
        &mut self,
        name: &str,
        command_name: &str,
        command: u64,
        args: SyscallArgs,
    ) {
        Self::syscall_log_unimplemented_command(self, name, command_name, command, args)
    }
}
