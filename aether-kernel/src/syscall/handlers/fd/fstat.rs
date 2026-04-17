use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::KernelSyscallContext;
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct FstatSyscall => nr::FSTAT, "fstat", |ctx, args| {
        SyscallDisposition::Return(ctx.fstat(args.get(0), args.get(1)))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_fstat(&mut self, fd: u64, address: u64) -> SysResult<u64> {
        let descriptor = self.process.files.get(fd as u32).ok_or(SysErr::BadFd)?;
        let node = descriptor.file.lock().node();
        let stat = crate::fs::make_stat(&node);
        let bytes = crate::fs::serialize_stat(&stat);
        self.write_user_buffer(address, &bytes)?;
        Ok(0)
    }
}
