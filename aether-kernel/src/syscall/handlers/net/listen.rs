use crate::arch::syscall::nr;
use crate::errno::SysResult;
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(pub struct ListenSyscall => nr::LISTEN, "listen", |ctx, args| {
    SyscallDisposition::Return(ctx.listen(args.get(0), args.get(1) as i32))
});

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_listen(&mut self, fd: u64, backlog: i32) -> SysResult<u64> {
        let (_file_ref, socket) = self.socket_from_fd(fd)?;
        socket.listen(backlog)?;
        Ok(0)
    }
}
