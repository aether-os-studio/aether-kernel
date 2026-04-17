use crate::arch::syscall::nr;

crate::declare_syscall!(pub struct SendmsgSyscall => nr::SENDMSG, "sendmsg", |ctx, args| {
    ctx.sendmsg_blocking(args.get(0), args.get(1), args.get(2))
});
