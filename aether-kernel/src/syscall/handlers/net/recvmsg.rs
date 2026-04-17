use crate::arch::syscall::nr;

crate::declare_syscall!(pub struct RecvmsgSyscall => nr::RECVMSG, "recvmsg", |ctx, args| {
    ctx.recvmsg_blocking(args.get(0), args.get(1), args.get(2))
});
