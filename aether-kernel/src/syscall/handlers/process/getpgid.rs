use crate::arch::syscall::nr;
use crate::errno::SysResult;
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct GetPgidSyscall => nr::GETPGID, "getpgid", |ctx, _args| {
        SyscallDisposition::Return(ctx.getpgid())
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_getpgid(&self) -> SysResult<u64> {
        Ok(self.process.identity.process_group as u64)
    }
}
