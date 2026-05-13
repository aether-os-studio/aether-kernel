use crate::arch::syscall::nr;
use crate::declare_syscall;
use crate::errno::SysResult;
use crate::process::ProcessSyscallContext;
use crate::syscall::SyscallDisposition;

declare_syscall! {
    pub struct FsyncSyscall => nr::FSYNC, "fsync", |ctx, args| {
        SyscallDisposition::Return(ctx.fsync(args.get(0), args.get(1), args.get(2)))
    }
}

impl ProcessSyscallContext<'_> {
    #![allow(unused)]
    pub(crate) fn fsync(&mut self, fd: u64, datasync: u64, flags: u64) -> SysResult<u64> {
        SysResult::Ok(0) // Placeholder return value
    }
}
