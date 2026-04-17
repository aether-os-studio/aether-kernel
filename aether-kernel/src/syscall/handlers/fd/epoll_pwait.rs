use crate::arch::syscall::nr;
crate::declare_syscall!(
    pub struct EpollPwaitSyscall => nr::EPOLL_PWAIT, "epoll_pwait", |ctx, args| {
        ctx.epoll_pwait_blocking(
            args.get(0),
            args.get(1),
            args.get(2) as usize,
            args.get(3) as i32,
            args.get(4),
        )
    }
);
