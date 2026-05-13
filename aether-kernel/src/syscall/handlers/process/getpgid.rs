use crate::arch::syscall::nr;
use crate::errno::SysResult;
use crate::process::ProcessSyscallContext;
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct GetPgidSyscall => nr::GETPGID, "getpgid", |ctx, _args| {
        SyscallDisposition::Return(ctx.getpgid())
    }
);

impl ProcessSyscallContext<'_> {
    pub(crate) fn getpgid(&self) -> SysResult<u64> {
        Ok(self.process.identity.process_group as u64)
    }
}
