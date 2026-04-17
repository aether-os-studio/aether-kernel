use crate::arch::syscall::nr;
use crate::declare_syscall;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

declare_syscall! {
    pub struct DupSyscall => nr::DUP, "dup", |ctx, args| {
        SyscallDisposition::Return(ctx.dup(args.get(0)))
    }
}

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_dup(&mut self, fd: u64) -> SysResult<u64> {
        self.process
            .files
            .duplicate(fd as u32, 0, false)
            .map(u64::from)
            .ok_or(SysErr::BadFd)
    }
}
