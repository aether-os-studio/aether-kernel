use crate::arch::syscall::nr;
use crate::errno::SysResult;
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(pub struct ChmodSyscall => nr::CHMOD, "chmod", |ctx, args| {
    match crate::syscall::abi::read_path(ctx, args.get(0), 4096) {
        Ok(path) => SyscallDisposition::Return(ctx.chmod(path.as_str(), args.get(1))),
        Err(error) => SyscallDisposition::Return(Err(error)),
    }
});

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_chmod(&mut self, path: &str, mode: u64) -> SysResult<u64> {
        self.syscall_fchmodat(-100, path, mode)
    }
}
