use crate::arch::syscall::nr;
use aether_vfs::NodeKind;

use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::KernelSyscallContext;
use crate::syscall::SyscallDisposition;
use crate::syscall::abi::{arg_i64_from_i32, read_path};

crate::declare_syscall!(
    pub struct ReadlinkAtSyscall => nr::READLINKAT, "readlinkat", |ctx, args| {
        let Ok(path) = read_path(ctx, args.get(1), 512) else {
            return SyscallDisposition::Return(Err(crate::errno::SysErr::Fault));
        };
        SyscallDisposition::Return(ctx.readlinkat(
            arg_i64_from_i32(args.get(0)),
            &path,
            args.get(2),
            args.get(3) as usize,
        ))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_readlinkat(
        &mut self,
        dirfd: i64,
        path: &str,
        address: u64,
        len: usize,
    ) -> SysResult<u64> {
        if path.is_empty() {
            return Err(SysErr::NoEnt);
        }

        let fs_view = self.fs_view_for_dirfd(dirfd, path)?;
        let (node, _) = self
            .services
            .lookup_node_with_identity(&fs_view, path, false)?;
        if node.kind() != NodeKind::Symlink {
            return Err(SysErr::Inval);
        }

        let target = node.symlink_target().ok_or(SysErr::Inval)?;
        let bytes = target.as_bytes();
        let count = core::cmp::min(len, bytes.len());
        self.write_user_buffer(address, &bytes[..count])?;
        Ok(count as u64)
    }
}
