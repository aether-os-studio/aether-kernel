use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(pub struct ShutdownSyscall => nr::SHUTDOWN, "shutdown", |ctx, args| {
    SyscallDisposition::Return(ctx.shutdown(args.get(0), args.get(1)))
});

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_shutdown(&mut self, fd: u64, how: u64) -> SysResult<u64> {
        let (_file_ref, socket) = self.socket_from_fd(fd)?;
        socket.shutdown(i32::try_from(how).map_err(|_| SysErr::Inval)?)?;
        Ok(0)
    }
}
