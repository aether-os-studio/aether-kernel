use crate::arch::syscall::nr;
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct TgkillSyscall => nr::TGKILL, "tgkill", |ctx, args| {
        SyscallDisposition::Return(ctx.send_signal(args.get(1) as i32, args.get(2)))
    }
);
