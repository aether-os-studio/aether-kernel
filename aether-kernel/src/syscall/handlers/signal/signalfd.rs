use crate::arch::syscall::nr;
use crate::errno::SysResult;
use crate::process::ProcessSyscallContext;
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(pub struct SignalfdSyscall => nr::SIGNALFD, "signalfd", |ctx, args| {
    SyscallDisposition::Return(ctx.signalfd(args.get(0) as i32, args.get(1), args.get(2) as usize))
});

impl ProcessSyscallContext<'_> {
    pub(crate) fn signalfd(&mut self, fd: i32, mask: u64, sigsetsize: usize) -> SysResult<u64> {
        self.signalfd4(fd, mask, sigsetsize, 0)
    }
}
