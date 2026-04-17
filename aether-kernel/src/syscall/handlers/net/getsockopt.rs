use alloc::vec::Vec;

use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;
use crate::syscall::handlers::socket_common::write_length_result;

crate::declare_syscall!(
    pub struct GetsockoptSyscall => nr::GETSOCKOPT, "getsockopt", |ctx, args| {
        SyscallDisposition::Return(
            ctx.getsockopt_value(args.get(0), args.get(1), args.get(2))
                .and_then(|value| write_length_result(ctx, args.get(3), args.get(4), &value)),
        )
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_getsockopt_value(
        &mut self,
        fd: u64,
        level: u64,
        optname: u64,
    ) -> SysResult<Vec<u8>> {
        let (_file_ref, socket) = self.socket_from_fd(fd)?;
        socket.getsockopt(
            i32::try_from(level).map_err(|_| SysErr::Inval)?,
            i32::try_from(optname).map_err(|_| SysErr::Inval)?,
        )
    }
}
