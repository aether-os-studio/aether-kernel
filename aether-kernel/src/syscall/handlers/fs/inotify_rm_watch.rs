use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct InotifyRmWatchSyscall => nr::INOTIFY_RM_WATCH, "inotify_rm_watch", |ctx, args| {
        SyscallDisposition::Return(ctx.inotify_rm_watch(args.get(0), args.get(1) as i32))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_inotify_rm_watch(&mut self, fd: u64, wd: i32) -> SysResult<u64> {
        let descriptor = self.process.files.get(fd as u32).ok_or(SysErr::BadFd)?;
        let file = descriptor.file.lock();
        let inotify = file
            .file_ops()
            .and_then(|ops| ops.as_any().downcast_ref::<crate::fs::InotifyFile>())
            .ok_or(SysErr::Inval)?;
        inotify.remove_watch(wd).map_err(SysErr::from)?;
        Ok(0)
    }
}
