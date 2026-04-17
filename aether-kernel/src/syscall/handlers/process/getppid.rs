use crate::arch::syscall::nr;
use crate::errno::SysResult;
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct GetPpidSyscall => nr::GETPPID, "getppid", |ctx, _args| {
        SyscallDisposition::Return(ctx.getppid())
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_getppid(&self) -> SysResult<u64> {
        Ok(self.process.identity.parent.unwrap_or(0) as u64)
    }
}
