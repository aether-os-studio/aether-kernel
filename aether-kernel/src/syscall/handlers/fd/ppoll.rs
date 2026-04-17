use crate::arch::syscall::nr;

crate::declare_syscall!(pub struct PpollSyscall => nr::PPOLL, "ppoll", |ctx, args| {
    ctx.ppoll_blocking(
        args.get(0),
        args.get(1) as usize,
        args.get(2),
        args.get(3),
        args.get(4) as usize,
    )
});
