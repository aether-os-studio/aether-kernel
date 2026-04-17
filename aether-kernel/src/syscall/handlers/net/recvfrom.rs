use crate::arch::syscall::nr;

crate::declare_syscall!(pub struct RecvfromSyscall => nr::RECVFROM, "recvfrom", |ctx, args| {
    ctx.recvfrom_blocking(
        args.get(0),
        args.get(1),
        args.get(2) as usize,
        args.get(3),
        args.get(4),
        args.get(5),
    )
});
