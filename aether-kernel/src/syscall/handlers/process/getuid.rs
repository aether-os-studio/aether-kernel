use crate::arch::syscall::nr;
use crate::errno::SysResult;
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct GetUidSyscall => nr::GETUID, "getuid", |ctx, _args| {
        SyscallDisposition::Return(ctx.getuid())
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_getuid(&self) -> SysResult<u64> {
        Ok(self.process.credentials.uid as u64)
    }
}
