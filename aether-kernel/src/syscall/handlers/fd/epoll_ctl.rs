use crate::arch::syscall::nr;
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct EpollCtlSyscall => nr::EPOLL_CTL, "epoll_ctl", |ctx, args| {
        let epfd = args.get(0);
        let op = args.get(1) as i32;
        let fd = args.get(2);
        let event = args.get(3);
        SyscallDisposition::Return(ctx.epoll_ctl(epfd, op, fd, event))
    }
);
