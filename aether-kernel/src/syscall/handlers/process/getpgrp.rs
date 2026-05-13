use crate::arch::syscall::nr;
use crate::errno::SysResult;
use crate::process::ProcessSyscallContext;
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct GetPgrpSyscall => nr::GETPGRP, "getpgrp", |ctx, _args| {
        SyscallDisposition::Return(ctx.getpgrp())
    }
);

impl ProcessSyscallContext<'_> {
    pub(crate) fn getpgrp(&self) -> SysResult<u64> {
        Ok(self.process.identity.process_group as u64)
    }
}
