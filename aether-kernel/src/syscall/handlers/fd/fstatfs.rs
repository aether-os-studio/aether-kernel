use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::KernelSyscallContext;
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct FstatfsSyscall => nr::FSTATFS, "fstatfs", |ctx, args| {
        SyscallDisposition::Return(ctx.fstatfs(args.get(0), args.get(1)))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_fstatfs(&mut self, fd: u64, address: u64) -> SysResult<u64> {
        let descriptor = self.process.files.get(fd as u32).ok_or(SysErr::BadFd)?;
        let bytes = crate::fs::serialize_statfs(&descriptor.filesystem.statfs);
        self.write_user_buffer(address, &bytes)?;
        Ok(0)
    }
}
