use crate::arch::syscall::nr;
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct ExitSyscall => nr::EXIT, "exit", |_ctx, args| {
        SyscallDisposition::Exit(args.get(0) as i32)
    }
);
