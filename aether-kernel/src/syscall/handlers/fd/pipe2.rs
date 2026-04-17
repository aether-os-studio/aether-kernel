use crate::arch::syscall::nr;
use crate::declare_syscall;
use crate::syscall::SyscallDisposition;

declare_syscall! {
    pub struct Pipe2Syscall => nr::PIPE2, "pipe2", |ctx, args| {
        SyscallDisposition::Return(ctx.pipe(args.get(0), args.get(1)))
    }
}
