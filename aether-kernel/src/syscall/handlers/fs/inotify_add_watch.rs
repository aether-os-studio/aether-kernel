use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct InotifyAddWatchSyscall => nr::INOTIFY_ADD_WATCH, "inotify_add_watch", |ctx, args| {
        let Ok(path) = ctx.read_user_c_string(args.get(1), 512) else {
            return SyscallDisposition::Return(Err(crate::errno::SysErr::Fault));
        };
        SyscallDisposition::Return(ctx.inotify_add_watch(args.get(0), &path, args.get(2)))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_inotify_add_watch(
        &mut self,
        fd: u64,
        path: &str,
        mask: u64,
    ) -> SysResult<u64> {
        let mask = u32::try_from(mask).map_err(|_| SysErr::Inval)?;
        if (mask & !crate::fs::INOTIFY_ADD_WATCH_VALID_MASK) != 0 {
            return Err(SysErr::Inval);
        }
        if path.is_empty() {
            return Err(SysErr::NoEnt);
        }

        let follow_final = (mask & crate::fs::IN_DONT_FOLLOW) == 0;
        let (node, _) =
            self.services
                .lookup_node_with_identity(&self.process.fs, path, follow_final)?;
        if (mask & crate::fs::IN_ONLYDIR) != 0 && node.kind() != aether_vfs::NodeKind::Directory {
            return Err(SysErr::NotDir);
        }

        let descriptor = self.process.files.get(fd as u32).ok_or(SysErr::BadFd)?;
        let file = descriptor.file.lock();
        let inotify = file
            .file_ops()
            .and_then(|ops| ops.as_any().downcast_ref::<crate::fs::InotifyFile>())
            .ok_or(SysErr::Inval)?;
        inotify
            .add_watch(&node, mask)
            .map(|wd| wd as u64)
            .map_err(SysErr::from)
    }
}
