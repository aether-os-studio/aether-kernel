use alloc::vec;

use crate::arch::syscall::nr;
use aether_vfs::{FsError, PollEvents};

use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext, read_iovec_array};
use crate::syscall::{KernelSyscallContext, SyscallDisposition};

crate::declare_syscall!(
    pub struct ReadvSyscall => nr::READV, "readv", |ctx, args| {
        ctx.readv_fd_blocking(args.get(0), args.get(1), args.get(2) as usize)
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_readv_fd(&mut self, fd: u64, iov: u64, iovcnt: usize) -> SysResult<u64> {
        const IOV_MAX: usize = 1024;

        let file_ref = self
            .process
            .files
            .get(fd as u32)
            .ok_or(SysErr::BadFd)?
            .file
            .clone();
        let segments =
            read_iovec_array(&self.process.task.address_space, iov, iovcnt.min(IOV_MAX))?;
        let mut file = file_ref.lock();
        let mut total = 0usize;

        for segment in segments {
            if segment.len == 0 {
                continue;
            }
            let mut buffer = vec![0u8; segment.len];
            let read = match file.read(&mut buffer) {
                Ok(read) => read,
                Err(FsError::WouldBlock) if !file.flags().nonblock() => return Err(SysErr::Again),
                Err(error) => return Err(SysErr::from(error)),
            };
            if read == 0 {
                break;
            }
            self.write_user_buffer(segment.base, &buffer[..read])?;
            total = total.saturating_add(read);
            if read < segment.len {
                break;
            }
        }

        Ok(total as u64)
    }

    pub(crate) fn syscall_readv_fd_blocking(
        &mut self,
        fd: u64,
        iov: u64,
        iovcnt: usize,
    ) -> SyscallDisposition {
        self.file_blocking_syscall(fd as u32, PollEvents::READ, |ctx| {
            ctx.syscall_readv_fd(fd, iov, iovcnt)
        })
    }
}
