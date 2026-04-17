use crate::arch::syscall::nr;
use crate::declare_syscall;
use crate::errno::SysResult;
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

declare_syscall! {
    pub struct Dup2Syscall => nr::DUP2, "dup2", |ctx, args| {
        SyscallDisposition::Return(ctx.dup2(args.get(0), args.get(1)))
    }
}

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_dup2(&mut self, oldfd: u64, newfd: u64) -> SysResult<u64> {
        self.dup_to(oldfd, newfd, 0, false)
    }
}
