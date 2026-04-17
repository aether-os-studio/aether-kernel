use crate::arch::syscall::nr;
use crate::errno::SysResult;
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct GetTidSyscall => nr::GETTID, "gettid", |ctx, _args| {
        SyscallDisposition::Return(ctx.gettid())
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_gettid(&self) -> SysResult<u64> {
        Ok(self.process.identity.pid as u64)
    }
}
