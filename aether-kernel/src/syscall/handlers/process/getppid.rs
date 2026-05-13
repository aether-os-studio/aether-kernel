use crate::arch::syscall::nr;
use crate::errno::SysResult;
use crate::process::ProcessSyscallContext;
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct GetPpidSyscall => nr::GETPPID, "getppid", |ctx, _args| {
        SyscallDisposition::Return(ctx.getppid())
    }
);

impl ProcessSyscallContext<'_> {
    pub(crate) fn getppid(&self) -> SysResult<u64> {
        Ok(self.process.identity.parent.unwrap_or(0) as u64)
    }
}
