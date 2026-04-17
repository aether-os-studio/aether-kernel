use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;
use crate::syscall::abi::read_path;
use aether_vfs::NodeRef;

crate::declare_syscall!(
    pub struct AccessSyscall => nr::ACCESS, "access", |ctx, args| {
        let Ok(path) = read_path(ctx, args.get(0), 512) else {
            return SyscallDisposition::Return(Err(crate::errno::SysErr::Fault));
        };
        SyscallDisposition::Return(ctx.access(&path, args.get(1)))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_access(&mut self, path: &str, mode: u64) -> SysResult<u64> {
        self.syscall_faccessat(-100, path, mode, 0)
    }

    pub(crate) fn syscall_faccessat(
        &mut self,
        dirfd: i64,
        path: &str,
        mode: u64,
        flags: u64,
    ) -> SysResult<u64> {
        const AT_FDCWD: i64 = -100;
        const AT_EACCESS: u64 = 0x200;
        const AT_SYMLINK_NOFOLLOW: u64 = 0x100;
        const AT_EMPTY_PATH: u64 = 0x1000;
        const VALID_FLAGS: u64 = AT_EACCESS | AT_SYMLINK_NOFOLLOW | AT_EMPTY_PATH;
        const F_OK: u64 = 0;
        const X_OK: u64 = 1;
        const W_OK: u64 = 2;
        const R_OK: u64 = 4;

        if (flags & !VALID_FLAGS) != 0 || (mode & !(F_OK | X_OK | W_OK | R_OK)) != 0 {
            return Err(SysErr::Inval);
        }

        let follow_final = (flags & AT_SYMLINK_NOFOLLOW) == 0;
        let node = if (flags & AT_EMPTY_PATH) != 0 && path.is_empty() {
            if dirfd == AT_FDCWD {
                self.process.fs.cwd_node()
            } else {
                let descriptor = self.process.files.get(dirfd as u32).ok_or(SysErr::BadFd)?;
                descriptor.file.lock().node()
            }
        } else {
            if path.is_empty() {
                return Err(SysErr::NoEnt);
            }
            let fs_view = self.fs_view_for_dirfd(dirfd, path)?;
            let (node, _) =
                self.services
                    .lookup_node_with_identity(&fs_view, path, follow_final)?;
            node
        };

        check_access_mode(&node, mode)
    }
}

fn check_access_mode(node: &NodeRef, mode: u64) -> SysResult<u64> {
    const X_OK: u64 = 1;
    const W_OK: u64 = 2;
    const R_OK: u64 = 4;

    let metadata = node.metadata();
    let permission_bits = metadata.mode & 0o777;

    if mode == 0 {
        return Ok(0);
    }

    if (mode & (R_OK | W_OK)) != 0 && (mode & X_OK) == 0 {
        return Ok(0);
    }

    if (mode & X_OK) != 0 && (permission_bits & 0o111) == 0 {
        return Err(SysErr::Access);
    }

    Ok(0)
}
