use crate::arch::syscall::nr;
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct GetPidSyscall => nr::GETPID, "getpid", |ctx, _args| {
        SyscallDisposition::Return(Ok(ctx.pid() as u64))
    }
);
