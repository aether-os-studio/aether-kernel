use crate::arch::syscall::nr;
use crate::errno::SysResult;
use crate::process::ProcessSyscallContext;
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(pub struct LchownSyscall => nr::LCHOWN, "lchown", |ctx, args| {
    match crate::syscall::abi::read_path(ctx, args.get(0), 4096) {
        Ok(path) => SyscallDisposition::Return(ctx.lchown(path.as_str(), args.get(1), args.get(2))),
        Err(error) => SyscallDisposition::Return(Err(error)),
    }
});

impl ProcessSyscallContext<'_> {
    pub(crate) fn lchown(&mut self, path: &str, owner: u64, group: u64) -> SysResult<u64> {
        self.fchownat(
            crate::syscall::abi::AT_FDCWD,
            path,
            owner,
            group,
            crate::syscall::abi::AT_SYMLINK_NOFOLLOW,
        )
    }
}
