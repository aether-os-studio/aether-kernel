use crate::arch::syscall::nr;
use crate::errno::SysResult;
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;
use crate::syscall::abi::{arg_i64_from_i32, read_path};

crate::declare_syscall!(
    pub struct OpenAtSyscall => nr::OPENAT, "openat", |ctx, args| {
        let dirfd = arg_i64_from_i32(args.get(0));
        let Ok(path) = read_path(ctx, args.get(1), 512) else {
            return SyscallDisposition::Return(Err(crate::errno::SysErr::Fault));
        };
        SyscallDisposition::Return(ctx.openat(dirfd, &path, args.get(2), args.get(3)))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_openat(
        &mut self,
        dirfd: i64,
        path: &str,
        flags: u64,
        mode: u64,
    ) -> SysResult<u64> {
        const O_CREAT: u64 = 0o100;
        const O_EXCL: u64 = 0o200;
        let fs_view = self.fs_view_for_dirfd(dirfd, path)?;

        match self
            .services
            .lookup_node_with_identity(&fs_view, path, true)
        {
            Ok(existing) => {
                if (flags & O_CREAT) != 0 && (flags & O_EXCL) != 0 {
                    return Err(crate::errno::SysErr::Exists);
                }
                let location = self.location_for_lookup(&fs_view, path, &existing.0);
                self.open_node(existing.0, existing.1, location, flags)
            }
            Err(crate::errno::SysErr::NoEnt) if (flags & O_CREAT) != 0 => {
                let created =
                    self.services
                        .create_file(&fs_view, path, self.masked_mode(mode, 0o100000))?;
                let location = self.location_for_lookup(&fs_view, path, &created.0);
                self.open_node(created.0, created.1, location, flags)
            }
            Err(error) => Err(error),
        }
    }
}
