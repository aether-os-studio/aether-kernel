use crate::arch::syscall::nr;
use crate::declare_syscall;
use crate::errno::SysResult;
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

declare_syscall! {
    pub struct Dup3Syscall => nr::DUP3, "dup3", |ctx, args| {
        SyscallDisposition::Return(ctx.dup3(args.get(0), args.get(1), args.get(2)))
    }
}

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_dup3(&mut self, oldfd: u64, newfd: u64, flags: u64) -> SysResult<u64> {
        self.dup_to(oldfd, newfd, flags, true)
    }
}
