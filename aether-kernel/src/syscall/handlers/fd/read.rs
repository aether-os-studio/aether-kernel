use alloc::vec;

use crate::arch::syscall::nr;
use aether_vfs::{FsError, PollEvents};

use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::{KernelSyscallContext, SyscallDisposition};

crate::declare_syscall!(
    pub struct ReadSyscall => nr::READ, "read", |ctx, args| {
        ctx.read_fd_blocking(args.get(0), args.get(1), args.get(2) as usize)
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_read_fd_blocking(
        &mut self,
        fd: u64,
        address: u64,
        len: usize,
    ) -> SyscallDisposition {
        self.file_blocking_syscall(fd as u32, PollEvents::READ, |ctx| {
            ctx.syscall_read_fd(fd, address, len)
        })
    }

    pub(crate) fn syscall_read_fd(&mut self, fd: u64, address: u64, len: usize) -> SysResult<u64> {
        let descriptor = self.process.files.get(fd as u32).ok_or(SysErr::BadFd)?;
        let file_ref = descriptor.file.clone();
        let nonblock = file_ref.lock().flags().nonblock();
        let mut bytes = vec![0; len];
        let read = match file_ref.lock().read(&mut bytes) {
            Ok(read) => read,
            Err(FsError::WouldBlock) if !nonblock => return Err(SysErr::Again),
            Err(error) => return Err(SysErr::from(error)),
        };
        bytes.truncate(read);
        self.write_user_buffer(address, &bytes)?;
        Ok(bytes.len() as u64)
    }
}
