use crate::arch::syscall::nr;
use crate::syscall::SyscallDisposition;
use crate::syscall::abi::{arg_i64_from_i32, read_path};

crate::declare_syscall!(
    pub struct FaccessAtSyscall => nr::FACCESSAT, "faccessat", |ctx, args| {
        let dirfd = arg_i64_from_i32(args.get(0));
        let Ok(path) = read_path(ctx, args.get(1), 512) else {
            return SyscallDisposition::Return(Err(crate::errno::SysErr::Fault));
        };
        SyscallDisposition::Return(ctx.faccessat(dirfd, &path, args.get(2), 0))
    }
);
