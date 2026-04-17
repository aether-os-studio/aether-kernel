use crate::arch::syscall::nr;
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(pub struct SignalfdSyscall => nr::SIGNALFD, "signalfd", |ctx, args| {
    SyscallDisposition::Return(ctx.signalfd(args.get(0) as i32, args.get(1), args.get(2) as usize))
});
