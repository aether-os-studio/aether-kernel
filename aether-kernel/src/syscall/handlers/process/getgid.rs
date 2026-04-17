use crate::arch::syscall::nr;
use crate::errno::SysResult;
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct GetGidSyscall => nr::GETGID, "getgid", |ctx, _args| {
        SyscallDisposition::Return(ctx.getgid())
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_getgid(&self) -> SysResult<u64> {
        Ok(self.process.credentials.gid as u64)
    }
}
