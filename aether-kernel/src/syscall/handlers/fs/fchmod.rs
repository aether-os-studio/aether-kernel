use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(pub struct FchmodSyscall => nr::FCHMOD, "fchmod", |ctx, args| {
    SyscallDisposition::Return(ctx.fchmod(args.get(0), args.get(1)))
});

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_fchmod(&mut self, fd: u64, mode: u64) -> SysResult<u64> {
        let descriptor = self.process.files.get(fd as u32).ok_or(SysErr::BadFd)?;
        let location = descriptor.location.clone().ok_or(SysErr::Inval)?;
        location
            .node()
            .set_mode(self.masked_mode(mode, location.node().metadata().mode))
            .map_err(SysErr::from)?;
        crate::fs::notify_attrib(&location.node());
        Ok(0)
    }
}
