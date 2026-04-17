use crate::arch::syscall::nr;
use crate::errno::SysResult;
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct RtSigreturnSyscall => nr::RT_SIGRETURN, "rt_sigreturn", |ctx, _args| {
        SyscallDisposition::Return(ctx.rt_sigreturn())
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_rt_sigreturn(&mut self) -> SysResult<u64> {
        crate::arch::restore_signal_from_user(self.process)
    }
}
