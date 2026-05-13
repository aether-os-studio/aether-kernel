use crate::arch::syscall::nr;
use crate::errno::SysResult;
use crate::process::ProcessSyscallContext;
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct GetUidSyscall => nr::GETUID, "getuid", |ctx, _args| {
        SyscallDisposition::Return(ctx.getuid())
    }
);

impl ProcessSyscallContext<'_> {
    pub(crate) fn getuid(&self) -> SysResult<u64> {
        Ok(self.process.credentials.uid as u64)
    }
}
