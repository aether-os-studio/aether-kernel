use alloc::vec::Vec;

use crate::errno::SysResult;
use crate::syscall::SyscallDisposition;

pub trait FdSyscallContext {
    fn close_fd(&mut self, fd: u64) -> SysResult<u64>;
    fn close_range(&mut self, first: u64, last: u64, flags: u64) -> SysResult<u64>;
    fn lseek(&mut self, fd: u64, offset: i64, whence: u64) -> SysResult<u64>;
    fn read_fd(&mut self, fd: u64, address: u64, len: usize) -> SysResult<u64>;
    fn read_fd_blocking(&mut self, fd: u64, address: u64, len: usize) -> SyscallDisposition;
    fn write_fd(&mut self, fd: u64, address: u64, len: usize) -> SysResult<u64>;
    fn write_fd_blocking(&mut self, fd: u64, address: u64, len: usize) -> SyscallDisposition;
    fn poll(&mut self, fds: u64, nfds: usize, timeout: i32) -> SysResult<u64>;
    fn poll_blocking(&mut self, fds: u64, nfds: usize, timeout: i32) -> SyscallDisposition;
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
    fn ioctl_fd(&mut self, fd: u64, command: u64, argument: u64) -> SysResult<u64>;
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
    fn sendmsg(&mut self, fd: u64, message: u64, flags: u64) -> SysResult<u64>;
    fn sendmsg_blocking(&mut self, fd: u64, message: u64, flags: u64) -> SyscallDisposition;
    fn bind(&mut self, fd: u64, address: u64, address_len: usize) -> SysResult<u64>;
    fn listen(&mut self, fd: u64, backlog: i32) -> SysResult<u64>;
    fn getdents64(&mut self, fd: u64, address: u64, len: usize) -> SysResult<u64>;
    fn pipe(&mut self, pipefd: u64, flags: u64) -> SysResult<u64>;
    fn eventfd(&mut self, initval: u32) -> SysResult<u64>;
    fn eventfd2(&mut self, initval: u32, flags: u64) -> SysResult<u64>;
    fn signalfd(&mut self, fd: i32, mask: u64, sigsetsize: usize) -> SysResult<u64>;
    fn signalfd4(&mut self, fd: i32, mask: u64, sigsetsize: usize, flags: u64) -> SysResult<u64>;
    fn inotify_init(&mut self) -> SysResult<u64>;
    fn inotify_init1(&mut self, flags: u64) -> SysResult<u64>;
    fn inotify_add_watch(&mut self, fd: u64, path: &str, mask: u64) -> SysResult<u64>;
    fn inotify_rm_watch(&mut self, fd: u64, wd: i32) -> SysResult<u64>;
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
}
