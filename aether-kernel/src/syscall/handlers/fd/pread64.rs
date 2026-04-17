use alloc::vec;

use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::KernelSyscallContext;
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct Pread64Syscall => nr::PREAD64, "pread64", |ctx, args| {
        SyscallDisposition::Return(ctx.pread64(
            args.get(0),
            args.get(1),
            args.get(2) as usize,
            args.get(3),
        ))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_pread64(
        &mut self,
        fd: u64,
        address: u64,
        len: usize,
        offset: u64,
    ) -> SysResult<u64> {
        let descriptor = self.process.files.get(fd as u32).ok_or(SysErr::BadFd)?;
        let node = descriptor.file.lock().node();
        let mut bytes = vec![0; len];
        let read = node
            .read(offset as usize, &mut bytes)
            .map_err(SysErr::from)?;
        bytes.truncate(read);
        self.write_user_buffer(address, &bytes)?;
        Ok(read as u64)
    }
}
