use crate::arch::syscall::nr;
use crate::syscall::SyscallDisposition;
use crate::syscall::abi::read_path;

crate::declare_syscall!(
    pub struct UnlinkSyscall => nr::UNLINK, "unlink", |ctx, args| {
        let Ok(path) = read_path(ctx, args.get(0), 512) else {
            return SyscallDisposition::Return(Err(crate::errno::SysErr::Fault));
        };
        SyscallDisposition::Return(ctx.unlinkat(-100, &path, 0))
    }
);
