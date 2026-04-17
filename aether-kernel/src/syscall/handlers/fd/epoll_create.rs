use crate::arch::syscall::nr;
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct EpollCreateSyscall => nr::EPOLL_CREATE, "epoll_create", |ctx, args| {
        SyscallDisposition::Return(ctx.epoll_create(args.get(0)))
    }
);

crate::declare_syscall!(
    pub struct EpollCreate1Syscall => nr::EPOLL_CREATE1, "epoll_create1", |ctx, args| {
        SyscallDisposition::Return(ctx.epoll_create1(args.get(0)))
    }
);
