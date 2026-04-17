use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::KernelSyscallContext;
use crate::syscall::SyscallDisposition;
use crate::syscall::abi::{arg_i64_from_i32, read_path_allow_empty};

crate::declare_syscall!(
    pub struct NewFstatAtSyscall => nr::NEWFSTATAT, "newfstatat", |ctx, args| {
        let dirfd = arg_i64_from_i32(args.get(0));
        let Ok(path) = read_path_allow_empty(ctx, args.get(1), args.get(3), 512) else {
            return SyscallDisposition::Return(Err(crate::errno::SysErr::Fault));
        };
        SyscallDisposition::Return(ctx.newfstatat(dirfd, &path, args.get(2), args.get(3)))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_newfstatat(
        &mut self,
        dirfd: i64,
        path: &str,
        address: u64,
        flags: u64,
    ) -> SysResult<u64> {
        const AT_SYMLINK_NOFOLLOW: u64 = 0x100;
        const AT_EMPTY_PATH: u64 = 0x1000;
        const VALID_FLAGS: u64 = AT_SYMLINK_NOFOLLOW | AT_EMPTY_PATH;

        if (flags & !VALID_FLAGS) != 0 {
            return Err(SysErr::Inval);
        }

        let node = if (flags & AT_EMPTY_PATH) != 0 && path.is_empty() {
            let descriptor = self.process.files.get(dirfd as u32).ok_or(SysErr::BadFd)?;
            descriptor.file.lock().node()
        } else {
            let fs_view = self.fs_view_for_dirfd(dirfd, path)?;
            let (node, _) = self.services.lookup_node_with_identity(
                &fs_view,
                path,
                (flags & AT_SYMLINK_NOFOLLOW) == 0,
            )?;
            node
        };
        let stat = crate::fs::make_stat(&node);
        let bytes = crate::fs::serialize_stat(&stat);
        self.write_user_buffer(address, &bytes)?;
        Ok(0)
    }
}
