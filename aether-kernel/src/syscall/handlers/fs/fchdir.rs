use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;
use aether_vfs::NodeKind;

crate::declare_syscall!(pub struct FchdirSyscall => nr::FCHDIR, "fchdir", |ctx, args| {
    SyscallDisposition::Return(ctx.fchdir(args.get(0)))
});

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_fchdir(&mut self, fd: u64) -> SysResult<u64> {
        let descriptor = self.process.files.get(fd as u32).ok_or(SysErr::BadFd)?;
        let location = descriptor.location.clone().ok_or(SysErr::NotDir)?;
        if location.node().kind() != NodeKind::Directory {
            return Err(SysErr::NotDir);
        }
        self.process.fs.set_cwd_location(location);
        Ok(0)
    }
}
