use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct FallocateSyscall => nr::FALLOCATE, "fallocate", |ctx, args| {
        SyscallDisposition::Return(ctx.fallocate(
            args.get(0),
            args.get(1),
            args.get(2) as i64,
            args.get(3) as i64,
        ))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_fallocate(
        &mut self,
        fd: u64,
        mode: u64,
        offset: i64,
        len: i64,
    ) -> SysResult<u64> {
        if offset < 0 || len <= 0 {
            return Err(SysErr::Inval);
        }

        let descriptor = self.process.files.get(fd as u32).ok_or(SysErr::BadFd)?;
        descriptor
            .file
            .lock()
            .fallocate(mode as u32, offset as u64, len as u64)
            .map_err(SysErr::from)?;
        Ok(0)
    }
}
