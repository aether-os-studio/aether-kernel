use alloc::vec::Vec;

use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct SetsockoptSyscall => nr::SETSOCKOPT, "setsockopt", |ctx, args| {
        SyscallDisposition::Return(
            ctx.setsockopt(args.get(0), args.get(1), args.get(2), args.get(3), args.get(4)),
        )
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_setsockopt(
        &mut self,
        fd: u64,
        level: u64,
        optname: u64,
        optval: u64,
        optlen: u64,
    ) -> SysResult<u64> {
        let (_file_ref, socket) = self.socket_from_fd(fd)?;
        let optlen = usize::try_from(optlen).map_err(|_| SysErr::Inval)?;
        let value = if optlen == 0 {
            Vec::new()
        } else {
            if optval == 0 {
                return Err(SysErr::Fault);
            }
            self.syscall_read_user_exact_buffer(optval, optlen)?
        };
        socket.setsockopt(
            i32::try_from(level).map_err(|_| SysErr::Inval)?,
            i32::try_from(optname).map_err(|_| SysErr::Inval)?,
            value.as_slice(),
        )?;
        Ok(0)
    }
}
