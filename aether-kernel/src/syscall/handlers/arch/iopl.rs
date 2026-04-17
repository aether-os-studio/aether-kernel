use crate::arch::syscall::nr;
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct IoplSyscall => nr::IOPL, "iopl", |ctx, args| {
        let level = args.get(0);
        SyscallDisposition::Return(ctx.iopl(level))
    }
);
