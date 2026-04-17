use crate::arch::syscall::nr;
use crate::errno::SysResult;
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(pub struct GetpeernameSyscall => nr::GETPEERNAME, "getpeername", |ctx, args| {
    SyscallDisposition::Return(ctx.getpeername(args.get(0), args.get(1), args.get(2)))
});

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_getpeername(
        &mut self,
        fd: u64,
        address: u64,
        address_len: u64,
    ) -> SysResult<u64> {
        let (_file_ref, socket) = self.socket_from_fd(fd)?;
        let name = socket.peer_name()?;
        self.write_returned_socket_address(address, address_len, Some(name.as_slice()))?;
        Ok(0)
    }
}
