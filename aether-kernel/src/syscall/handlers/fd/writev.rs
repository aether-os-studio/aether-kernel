use crate::arch::syscall::nr;
use aether_vfs::{FsError, PollEvents};

use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext, read_iovec_array};
use crate::syscall::{KernelSyscallContext, SyscallDisposition};

crate::declare_syscall!(
    pub struct WritevSyscall => nr::WRITEV, "writev", |ctx, args| {
        ctx.writev_fd_blocking(args.get(0), args.get(1), args.get(2) as usize)
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_writev_fd(&mut self, fd: u64, iov: u64, iovcnt: usize) -> SysResult<u64> {
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
        let mut total = 0usize;

        for segment in segments {
            if segment.len == 0 {
                continue;
            }
            let bytes = self.read_user_buffer(segment.base, segment.len)?;
            let nonblock = file_ref.lock().flags().nonblock();
            let written = match file_ref.lock().write(&bytes) {
                Ok(written) => written,
                Err(FsError::WouldBlock) if !nonblock => return Err(SysErr::Again),
                Err(error) => return Err(SysErr::from(error)),
            };
            total = total.saturating_add(written);
            if written < bytes.len() {
                break;
            }
        }

        Ok(total as u64)
    }

    pub(crate) fn syscall_writev_fd_blocking(
        &mut self,
        fd: u64,
        iov: u64,
        iovcnt: usize,
    ) -> SyscallDisposition {
        self.file_blocking_syscall(fd as u32, PollEvents::WRITE, |ctx| {
            ctx.syscall_writev_fd(fd, iov, iovcnt)
        })
    }
}
