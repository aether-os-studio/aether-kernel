use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct CloseSyscall => nr::CLOSE, "close", |ctx, args| { SyscallDisposition::Return(ctx.close_fd(args.get(0))) }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_close_fd(&mut self, fd: u64) -> SysResult<u64> {
        if self.process.files.close(fd as u32) {
            Ok(0)
        } else {
            Err(SysErr::BadFd)
        }
    }
}
