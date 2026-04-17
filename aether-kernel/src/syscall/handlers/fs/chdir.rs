use crate::arch::syscall::nr;
use crate::errno::SysErr;
use crate::errno::SysResult;
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;
use crate::syscall::abi::read_path;

crate::declare_syscall!(
    pub struct ChdirSyscall => nr::CHDIR, "chdir", |ctx, args| {
        let Ok(path) = read_path(ctx, args.get(0), 512) else {
            return SyscallDisposition::Return(Err(SysErr::Fault));
        };
        SyscallDisposition::Return(ctx.chdir(&path))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_chdir(&mut self, path: &str) -> SysResult<u64> {
        self.services.chdir(&mut self.process.fs, path)
    }
}
