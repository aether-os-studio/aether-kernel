use crate::arch::syscall::nr;

crate::declare_syscall!(pub struct Pselect6Syscall => nr::PSELECT6, "pselect6", |ctx, args| {
    ctx.pselect6_blocking(
        crate::syscall::abi::arg_i32(args.get(0)),
        args.get(1),
        args.get(2),
        args.get(3),
        args.get(4),
        args.get(5),
    )
});
