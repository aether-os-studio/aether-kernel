use crate::arch::syscall::nr;
use crate::errno::SysResult;
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;
use crate::syscall::abi::read_path;

crate::declare_syscall!(
    pub struct LinkSyscall => nr::LINK, "link", |ctx, args| {
        let old_path = match read_path(ctx, args.get(0), 4096) {
            Ok(path) => path,
            Err(error) => return SyscallDisposition::Return(Err(error)),
        };
        let new_path = match read_path(ctx, args.get(1), 4096) {
            Ok(path) => path,
            Err(error) => return SyscallDisposition::Return(Err(error)),
        };
        SyscallDisposition::Return(ctx.link(old_path.as_str(), new_path.as_str()))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_link(&mut self, old_path: &str, new_path: &str) -> SysResult<u64> {
        self.services.link(&self.process.fs, old_path, new_path, 0)
    }
}
