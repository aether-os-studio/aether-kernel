use crate::arch::syscall::nr;
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct ExitGroupSyscall => nr::EXIT_GROUP, "exit_group", |_ctx, args| {
        SyscallDisposition::Exit(args.get(0) as i32)
    }
);
