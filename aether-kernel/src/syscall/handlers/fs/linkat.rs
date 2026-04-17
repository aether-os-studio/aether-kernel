use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext, resolve_at_path};
use crate::syscall::SyscallDisposition;
use crate::syscall::abi::{arg_i64_from_i32, read_path};

crate::declare_syscall!(
    pub struct LinkAtSyscall => nr::LINKAT, "linkat", |ctx, args| {
        let old_path = match read_path(ctx, args.get(1), 4096) {
            Ok(path) => path,
            Err(error) => return SyscallDisposition::Return(Err(error)),
        };
        let new_path = match read_path(ctx, args.get(3), 4096) {
            Ok(path) => path,
            Err(error) => return SyscallDisposition::Return(Err(error)),
        };
        SyscallDisposition::Return(ctx.linkat(
            arg_i64_from_i32(args.get(0)),
            old_path.as_str(),
            arg_i64_from_i32(args.get(2)),
            new_path.as_str(),
            args.get(4),
        ))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_linkat(
        &mut self,
        olddirfd: i64,
        old_path: &str,
        newdirfd: i64,
        new_path: &str,
        flags: u64,
    ) -> SysResult<u64> {
        const AT_EMPTY_PATH: u64 = 0x1000;

        if (flags & AT_EMPTY_PATH) != 0 {
            // TODO: Linux AT_EMPTY_PATH links an existing fd target without path-walk.
            // The rootfs/vfs side still lacks that fd-backed source lookup path.
            return Err(SysErr::Inval);
        }
        if old_path.is_empty() && (flags & AT_EMPTY_PATH) == 0 {
            return Err(SysErr::NoEnt);
        }
        if new_path.is_empty() {
            return Err(SysErr::NoEnt);
        }

        let old_fs = self.fs_view_for_dirfd(olddirfd, old_path)?;
        let new_fs = self.fs_view_for_dirfd(newdirfd, new_path)?;
        let old_absolute = resolve_at_path(&old_fs, old_path);
        let new_absolute = resolve_at_path(&new_fs, new_path);
        self.services.link(
            &self.process.fs,
            old_absolute.as_str(),
            new_absolute.as_str(),
            flags,
        )
    }
}
