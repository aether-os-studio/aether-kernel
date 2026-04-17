use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext, resolve_at_path};
use crate::syscall::SyscallDisposition;
use crate::syscall::abi::arg_i64_from_i32;

crate::declare_syscall!(
    pub struct RenameAtSyscall => nr::RENAMEAT, "renameat", |ctx, args| {
        let Ok(old_path) = ctx.read_user_c_string(args.get(1), 512) else {
            return SyscallDisposition::Return(Err(crate::errno::SysErr::Fault));
        };
        let Ok(new_path) = ctx.read_user_c_string(args.get(3), 512) else {
            return SyscallDisposition::Return(Err(crate::errno::SysErr::Fault));
        };
        SyscallDisposition::Return(ctx.renameat(
            arg_i64_from_i32(args.get(0)),
            &old_path,
            arg_i64_from_i32(args.get(2)),
            &new_path,
        ))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_renameat(
        &mut self,
        olddirfd: i64,
        old_path: &str,
        newdirfd: i64,
        new_path: &str,
    ) -> SysResult<u64> {
        if old_path.is_empty() || new_path.is_empty() {
            return Err(SysErr::NoEnt);
        }

        let old_fs = self.fs_view_for_dirfd(olddirfd, old_path)?;
        let new_fs = self.fs_view_for_dirfd(newdirfd, new_path)?;
        let old_absolute = resolve_at_path(&old_fs, old_path);
        let new_absolute = resolve_at_path(&new_fs, new_path);
        self.services.rename(
            &self.process.fs,
            old_absolute.as_str(),
            new_absolute.as_str(),
        )
    }
}
