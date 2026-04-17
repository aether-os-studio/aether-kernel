use aether_vfs::FileAdvice;

use crate::arch::syscall::nr;
use crate::errno::SysErr;
use crate::syscall::SyscallDisposition;

fn validate_advice(raw: u64) -> Result<FileAdvice, SysErr> {
    match raw {
        0 => Ok(FileAdvice::Normal),
        1 => Ok(FileAdvice::Random),
        2 => Ok(FileAdvice::Sequential),
        3 => Ok(FileAdvice::WillNeed),
        4 => Ok(FileAdvice::DontNeed),
        5 => Ok(FileAdvice::NoReuse),
        _ => Err(SysErr::Inval),
    }
}

crate::declare_syscall!(
    pub struct Fadvise64Syscall => nr::FADVISE64, "fadvise64", |ctx, args| {
        SyscallDisposition::Return(validate_advice(args.get(3)).and_then(|_| {
            ctx.fadvise64(args.get(0), args.get(1), args.get(2), args.get(3))
        }))
    }
);
