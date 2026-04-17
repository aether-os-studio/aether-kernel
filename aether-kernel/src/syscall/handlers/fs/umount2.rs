use crate::arch::syscall::nr;
use crate::errno::SysErr;
use crate::syscall::SyscallDisposition;
use crate::syscall::abi::read_path;

crate::declare_syscall!(
    pub struct Umount2Syscall => nr::UMOUNT2, "umount2", |ctx, args| {
        let Ok(target) = read_path(ctx, args.get(0), 256) else {
            return SyscallDisposition::Return(Err(SysErr::Fault));
        };
        SyscallDisposition::Return(ctx.umount(&target, args.get(1)))
    }
);
