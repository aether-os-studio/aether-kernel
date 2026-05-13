use crate::arch::syscall::nr;
use crate::errno::SysResult;
use crate::process::ProcessSyscallContext;
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct UmaskSyscall => nr::UMASK, "umask", |ctx, args| {
        SyscallDisposition::Return(ctx.umask(args.get(0)))
    }
);

impl ProcessSyscallContext<'_> {
    pub(crate) fn umask(&mut self, mask: u64) -> SysResult<u64> {
        let old = self.process.umask as u64;
        self.process.umask = (mask & 0o777) as u16;
        Ok(old)
    }
}
