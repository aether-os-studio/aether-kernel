use crate::arch::syscall::nr;
use crate::errno::SysResult;
use crate::process::ProcessSyscallContext;
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct GetGidSyscall => nr::GETGID, "getgid", |ctx, _args| {
        SyscallDisposition::Return(ctx.getgid())
    }
);

impl ProcessSyscallContext<'_> {
    pub(crate) fn getgid(&self) -> SysResult<u64> {
        Ok(self.process.credentials.gid as u64)
    }
}
