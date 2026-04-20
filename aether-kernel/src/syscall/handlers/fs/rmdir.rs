use crate::arch::syscall::nr;
use crate::syscall::SyscallDisposition;
use crate::syscall::abi::{AT_FDCWD, AT_REMOVEDIR, read_path};

crate::declare_syscall!(
    pub struct RmdirSyscall => nr::RMDIR, "rmdir", |ctx, args| {
        let Ok(path) = read_path(ctx, args.get(0), 512) else {
            return SyscallDisposition::Return(Err(crate::errno::SysErr::Fault));
        };
        SyscallDisposition::Return(ctx.unlinkat(AT_FDCWD, &path, AT_REMOVEDIR))
    }
);
