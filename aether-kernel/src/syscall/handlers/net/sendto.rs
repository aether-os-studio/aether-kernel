use crate::arch::syscall::nr;

crate::declare_syscall!(pub struct SendtoSyscall => nr::SENDTO, "sendto", |ctx, args| {
    ctx.sendto_blocking(
        args.get(0),
        args.get(1),
        args.get(2) as usize,
        args.get(3),
        args.get(4),
        args.get(5) as usize,
    )
});
