use crate::arch::syscall::nr;
use aether_vfs::PollEvents;

use crate::errno::SysResult;
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(pub struct ConnectSyscall => nr::CONNECT, "connect", |ctx, args| {
    ctx.connect_blocking(args.get(0), args.get(1), args.get(2) as usize)
});

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_connect(
        &mut self,
        fd: u64,
        address: u64,
        address_len: usize,
    ) -> SysResult<u64> {
        let (_file_ref, socket) = self.socket_from_fd(fd)?;
        let address = self.read_socket_address(address, address_len)?;
        socket.connect(address.as_slice())?;
        Ok(0)
    }

    pub(crate) fn syscall_connect_blocking(
        &mut self,
        fd: u64,
        address: u64,
        address_len: usize,
    ) -> SyscallDisposition {
        self.file_blocking_syscall(fd as u32, PollEvents::WRITE, |ctx| {
            ctx.syscall_connect(fd, address, address_len)
        })
    }
}
