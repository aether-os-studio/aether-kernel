use alloc::vec;

use crate::arch::syscall::nr;
use aether_vfs::{FsError, PollEvents};

use crate::errno::{SysErr, SysResult};
use crate::process::ProcessSyscallContext;
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct ReadSyscall => nr::READ, "read", |ctx, args| {
        ctx.read_fd_blocking(args.get(0), args.get(1), args.get(2) as usize)
    }
);

impl ProcessSyscallContext<'_> {
    pub(crate) fn read_fd_blocking(
        &mut self,
        fd: u64,
        address: u64,
        len: usize,
    ) -> SyscallDisposition {
        self.file_blocking_syscall(fd as u32, PollEvents::READ, |ctx| {
            ctx.read_fd(fd, address, len)
        })
    }

    pub(crate) fn read_fd(&mut self, fd: u64, address: u64, len: usize) -> SysResult<u64> {
        if let Ok((_file_ref, socket)) = self.socket_from_fd(fd) {
            let mut bytes = vec![0; len];
            let received = socket.recv_from(bytes.as_mut_slice(), 0)?;
            self.write_user_buffer(address, &bytes[..received.bytes_read])?;
            return Ok(received.bytes_read as u64);
        }

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
