use crate::arch::syscall::nr;
use crate::errno::SysResult;
use crate::process::ProcessSyscallContext;
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(pub struct InotifyInitSyscall => nr::INOTIFY_INIT, "inotify_init", |ctx, _args| {
    SyscallDisposition::Return(ctx.inotify_init())
});

impl ProcessSyscallContext<'_> {
    pub(crate) fn inotify_init(&mut self) -> SysResult<u64> {
        self.inotify_init1(0)
    }
}
