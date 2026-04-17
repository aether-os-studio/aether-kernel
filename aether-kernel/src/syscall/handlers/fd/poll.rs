use crate::arch::syscall::nr;

crate::declare_syscall!(pub struct PollSyscall => nr::POLL, "poll", |ctx, args| {
    ctx.poll_blocking(args.get(0), args.get(1) as usize, args.get(2) as i32)
});
