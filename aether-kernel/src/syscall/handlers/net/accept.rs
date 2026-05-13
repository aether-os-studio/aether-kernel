use crate::arch::syscall::nr;
use aether_vfs::PollEvents;

use crate::errno::SysResult;
use crate::process::ProcessSyscallContext;
use crate::syscall::SyscallDisposition;
use crate::syscall::handlers::socket_common::validate_accept4_flags;

crate::declare_syscall!(pub struct AcceptSyscall => nr::ACCEPT, "accept", |ctx, args| {
    ctx.accept_blocking(args.get(0), args.get(1), args.get(2))
});

crate::declare_syscall!(pub struct Accept4Syscall => nr::ACCEPT4, "accept4", |ctx, args| {
    let flags = args.get(3);
    match validate_accept4_flags(flags) {
        Ok(()) => ctx.accept4_blocking(args.get(0), args.get(1), args.get(2), flags),
        Err(error) => SyscallDisposition::err(error),
    }
});

impl ProcessSyscallContext<'_> {
    pub(crate) fn accept4(
        &mut self,
        fd: u64,
        address: u64,
        address_len: u64,
        flags: u64,
    ) -> SysResult<u64> {
        let (_file_ref, socket) = self.socket_from_fd(fd)?;
        let accepted = socket.accept()?;
        self.install_accepted_socket(accepted, address, address_len, flags)
    }

    pub(crate) fn accept_blocking(
        &mut self,
        fd: u64,
        address: u64,
        address_len: u64,
    ) -> SyscallDisposition {
        self.accept4_blocking(fd, address, address_len, 0)
    }

    pub(crate) fn accept4_blocking(
        &mut self,
        fd: u64,
        address: u64,
        address_len: u64,
        flags: u64,
    ) -> SyscallDisposition {
        self.file_blocking_syscall(fd as u32, PollEvents::READ, |ctx| {
            ctx.accept4(fd, address, address_len, flags)
        })
    }
}
