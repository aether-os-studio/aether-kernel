use crate::arch::syscall::nr;
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct TkillSyscall => nr::TKILL, "tkill", |ctx, args| {
        SyscallDisposition::Return(ctx.send_signal(args.get(0) as i32, args.get(1)))
    }
);
