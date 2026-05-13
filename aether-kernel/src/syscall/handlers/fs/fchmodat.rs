use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::ProcessSyscallContext;
use crate::syscall::SyscallDisposition;
use crate::syscall::abi::arg_i64_from_i32;

crate::declare_syscall!(pub struct FchmodatSyscall => nr::FCHMODAT, "fchmodat", |ctx, args| {
    match crate::syscall::abi::read_path(ctx, args.get(1), 4096) {
        Ok(path) => {
            SyscallDisposition::Return(ctx.fchmodat(
                arg_i64_from_i32(args.get(0)),
                path.as_str(),
                args.get(2),
            ))
        }
        Err(error) => SyscallDisposition::Return(Err(error)),
    }
});

impl ProcessSyscallContext<'_> {
    pub(crate) fn fchmodat(&mut self, dirfd: i64, path: &str, mode: u64) -> SysResult<u64> {
        if path.is_empty() {
            return Err(SysErr::NoEnt);
        }

        let fs_view = self.fs_view_for_dirfd(dirfd, path)?;
        let (node, _) = self
            .services
            .lookup_node_with_identity(&fs_view, path, true)?;
        let current_mode = node.metadata().mode;
        node.set_mode((current_mode & !0o7777) | ((mode as u32) & 0o7777))
            .map_err(SysErr::from)?;
        crate::fs::notify_attrib(&node);
        Ok(0)
    }
}
