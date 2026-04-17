use crate::arch::syscall::nr;
use crate::syscall::SyscallDisposition;
use crate::syscall::abi::{AT_FDCWD, read_path};

crate::declare_syscall!(pub struct ReadlinkSyscall => nr::READLINK, "readlink", |ctx, args| {
    let Ok(path) = read_path(ctx, args.get(0), 512) else {
        return SyscallDisposition::Return(Err(crate::errno::SysErr::Fault));
    };
    SyscallDisposition::Return(ctx.readlinkat(
        AT_FDCWD,
        &path,
        args.get(1),
        args.get(2) as usize,
    ))
});
