use crate::arch::syscall::nr;
use crate::errno::SysResult;
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;
use crate::syscall::abi::read_path;

crate::declare_syscall!(
    pub struct MkdirSyscall => nr::MKDIR, "mkdir", |ctx, args| {
        let Ok(path) = read_path(ctx, args.get(0), 512) else {
            return SyscallDisposition::Return(Err(crate::errno::SysErr::Fault));
        };
        SyscallDisposition::Return(ctx.mkdir(&path, args.get(1)))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_mkdir(&mut self, path: &str, mode: u64) -> SysResult<u64> {
        self.services
            .mkdir(&self.process.fs, path, self.masked_mode(mode, 0o040000))
    }
}
