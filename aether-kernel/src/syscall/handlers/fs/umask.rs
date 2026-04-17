use crate::arch::syscall::nr;
use crate::errno::SysResult;
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct UmaskSyscall => nr::UMASK, "umask", |ctx, args| {
        SyscallDisposition::Return(ctx.umask(args.get(0)))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_umask(&mut self, mask: u64) -> SysResult<u64> {
        let old = self.process.umask as u64;
        self.process.umask = (mask & 0o777) as u16;
        Ok(old)
    }
}
