use crate::arch::syscall::nr;
use crate::errno::SysResult;
use crate::process::{CloneParams, ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct ForkSyscall => nr::FORK, "fork", |ctx, _args| {
        SyscallDisposition::Return(ctx.fork(0))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_fork(&mut self, flags: u64) -> SysResult<u64> {
        let params = if flags == 0 {
            CloneParams::fork()
        } else {
            CloneParams::vfork()
        };
        self.syscall_clone_process(params)
    }
}
