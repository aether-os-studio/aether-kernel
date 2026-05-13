use crate::arch::syscall::nr;
use crate::errno::SysResult;
use crate::process::ProcessSyscallContext;
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct GetTidSyscall => nr::GETTID, "gettid", |ctx, _args| {
        SyscallDisposition::Return(ctx.gettid())
    }
);

impl ProcessSyscallContext<'_> {
    pub(crate) fn gettid(&self) -> SysResult<u64> {
        Ok(self.process.identity.pid as u64)
    }
}
