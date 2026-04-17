use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::KernelSyscallContext;
use crate::syscall::SyscallDisposition;
use crate::syscall::abi::arg_i64_from_i32;

crate::declare_syscall!(
    pub struct StatxSyscall => nr::STATX, "statx", |ctx, args| {
        const AT_EMPTY_PATH: u64 = 0x1000;
        let dirfd = arg_i64_from_i32(args.get(0));
        let pathname = args.get(1);
        let path = if pathname == 0 {
            if (args.get(2) & AT_EMPTY_PATH) == 0 {
                return SyscallDisposition::Return(Err(crate::errno::SysErr::Fault));
            }
            alloc::string::String::new()
        } else {
            let Ok(path) = ctx.read_user_c_string(pathname, 512) else {
                return SyscallDisposition::Return(Err(crate::errno::SysErr::Fault));
            };
            path
        };
        SyscallDisposition::Return(ctx.statx(
            dirfd,
            &path,
            args.get(2),
            args.get(3),
            args.get(4),
        ))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_statx(
        &mut self,
        dirfd: i64,
        path: &str,
        flags: u64,
        mask: u64,
        address: u64,
    ) -> SysResult<u64> {
        const AT_FDCWD: i64 = -100;
        const AT_SYMLINK_NOFOLLOW: u64 = 0x100;
        const AT_EMPTY_PATH: u64 = 0x1000;
        const AT_STATX_SYNC_TYPE: u64 = 0x6000;
        const AT_STATX_FORCE_SYNC: u64 = 0x2000;
        const AT_STATX_DONT_SYNC: u64 = 0x4000;
        const VALID_FLAGS: u64 =
            AT_SYMLINK_NOFOLLOW | AT_EMPTY_PATH | AT_STATX_FORCE_SYNC | AT_STATX_DONT_SYNC;

        if (flags & !VALID_FLAGS) != 0 || (mask as u32 & crate::fs::STATX_RESERVED) != 0 {
            return Err(SysErr::Inval);
        }
        if (flags & AT_STATX_SYNC_TYPE) == AT_STATX_SYNC_TYPE {
            return Err(SysErr::Inval);
        }

        let node = if (flags & AT_EMPTY_PATH) != 0 && path.is_empty() {
            if dirfd == AT_FDCWD {
                self.process.fs.cwd_node()
            } else {
                let descriptor = self.process.files.get(dirfd as u32).ok_or(SysErr::BadFd)?;
                descriptor.file.lock().node()
            }
        } else {
            let fs_view = self.fs_view_for_dirfd(dirfd, path)?;
            let (node, _) = self.services.lookup_node_with_identity(
                &fs_view,
                path,
                (flags & AT_SYMLINK_NOFOLLOW) == 0,
            )?;
            node
        };

        let statx = crate::fs::make_statx(&node, mask as u32);
        let bytes = crate::fs::serialize_statx(&statx);
        self.write_user_buffer(address, &bytes)?;
        Ok(0)
    }
}
