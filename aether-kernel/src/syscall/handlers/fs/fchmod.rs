use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::ProcessSyscallContext;
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(pub struct FchmodSyscall => nr::FCHMOD, "fchmod", |ctx, args| {
    SyscallDisposition::Return(ctx.fchmod(args.get(0), args.get(1)))
});

impl ProcessSyscallContext<'_> {
    pub(crate) fn fchmod(&mut self, fd: u64, mode: u64) -> SysResult<u64> {
        let descriptor = self.process.files.get(fd as u32).ok_or(SysErr::BadFd)?;
        let node = descriptor.file.lock().node();
        let current_mode = node.metadata().mode;
        node.set_mode((current_mode & !0o7777) | ((mode as u32) & 0o7777))
            .map_err(SysErr::from)?;
        crate::fs::notify_attrib(&node);
        Ok(0)
    }
}
