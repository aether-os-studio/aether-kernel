use crate::arch::syscall::nr;
use aether_vfs::{FsError, PollEvents};

use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::{KernelSyscallContext, SyscallDisposition};

crate::declare_syscall!(
    pub struct WriteSyscall => nr::WRITE, "write", |ctx, args| {
        ctx.write_fd_blocking(args.get(0), args.get(1), args.get(2) as usize)
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_write_fd_blocking(
        &mut self,
        fd: u64,
        address: u64,
        len: usize,
    ) -> SyscallDisposition {
        self.file_blocking_syscall(fd as u32, PollEvents::WRITE, |ctx| {
            ctx.syscall_write_fd(fd, address, len)
        })
    }

    pub(crate) fn syscall_write_fd(&mut self, fd: u64, address: u64, len: usize) -> SysResult<u64> {
        let bytes = self.read_user_buffer(address, len)?;
        let descriptor = self.process.files.get(fd as u32).ok_or(SysErr::BadFd)?;
        let file_ref = descriptor.file.clone();
        let nonblock = file_ref.lock().flags().nonblock();
        let written = match file_ref.lock().write(&bytes) {
            Ok(written) => written,
            Err(FsError::WouldBlock) if !nonblock => return Err(SysErr::Again),
            Err(error) => return Err(SysErr::from(error)),
        };
        Ok(written as u64)
    }
}
