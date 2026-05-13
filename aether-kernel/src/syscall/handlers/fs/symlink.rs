use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::ProcessSyscallContext;
use crate::syscall::SyscallDisposition;
use crate::syscall::abi::read_path;

crate::declare_syscall!(
    pub struct SymlinkSyscall => nr::SYMLINK, "symlink", |ctx, args| {
        let target = match read_path(ctx, args.get(0), 4096) {
            Ok(path) => path,
            Err(error) => return SyscallDisposition::Return(Err(error)),
        };
        let linkpath = match read_path(ctx, args.get(1), 4096) {
            Ok(path) => path,
            Err(error) => return SyscallDisposition::Return(Err(error)),
        };
        SyscallDisposition::Return(ctx.symlink(target.as_str(), linkpath.as_str()))
    }
);

impl ProcessSyscallContext<'_> {
    pub(crate) fn symlink(&mut self, target: &str, linkpath: &str) -> SysResult<u64> {
        if linkpath.is_empty() {
            return Err(SysErr::NoEnt);
        }
        self.services
            .create_symlink(&self.process.fs, linkpath, target)
    }
}
