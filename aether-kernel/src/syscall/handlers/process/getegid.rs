use crate::arch::syscall::nr;
use crate::errno::SysResult;
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct GeteGidSyscall => nr::GETEGID, "getegid", |ctx, _args| {
        SyscallDisposition::Return(ctx.getegid())
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_getegid(&self) -> SysResult<u64> {
        Ok(self.process.credentials.egid as u64)
    }
}
