use crate::arch::syscall::nr;
use crate::errno::SysResult;
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(pub struct InotifyInitSyscall => nr::INOTIFY_INIT, "inotify_init", |ctx, _args| {
    SyscallDisposition::Return(ctx.inotify_init())
});

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_inotify_init(&mut self) -> SysResult<u64> {
        self.syscall_inotify_init1(0)
    }
}
