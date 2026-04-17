use crate::arch::syscall::nr;
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct EpollWaitSyscall => nr::EPOLL_WAIT, "epoll_wait", |ctx, args| {
        let epfd = args.get(0);
        let events = args.get(1);
        let maxevents = args.get(2) as usize;
        let timeout = args.get(3) as i32;

        if maxevents == 0 {
            return SyscallDisposition::Return(Err(crate::errno::SysErr::Inval));
        }

        ctx.epoll_wait_blocking(epfd, events, maxevents, timeout)
    }
);
