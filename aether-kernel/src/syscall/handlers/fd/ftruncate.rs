use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct FtruncateSyscall => nr::FTRUNCATE, "ftruncate", |ctx, args| {
        SyscallDisposition::Return(ctx.ftruncate(args.get(0), args.get(1)))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_ftruncate(&mut self, fd: u64, length: u64) -> SysResult<u64> {
        let length = usize::try_from(length).map_err(|_| SysErr::Inval)?;
        let descriptor = self.process.files.get(fd as u32).ok_or(SysErr::BadFd)?;
        descriptor
            .file
            .lock()
            .node()
            .truncate(length)
            .map_err(SysErr::from)?;
        Ok(0)
    }
}
