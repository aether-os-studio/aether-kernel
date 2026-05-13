use crate::arch::syscall::nr;
use crate::errno::SysResult;
use crate::process::ProcessSyscallContext;
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct GeteUidSyscall => nr::GETEUID, "geteuid", |ctx, _args| {
        SyscallDisposition::Return(ctx.geteuid())
    }
);

impl ProcessSyscallContext<'_> {
    pub(crate) fn geteuid(&self) -> SysResult<u64> {
        Ok(self.process.credentials.euid as u64)
    }
}
