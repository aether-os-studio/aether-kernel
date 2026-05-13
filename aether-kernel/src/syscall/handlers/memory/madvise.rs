use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::ProcessSyscallContext;
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct MadviseSyscall => nr::MADVISE, "madvise", |ctx, args| {
        SyscallDisposition::Return(ctx.madvise(args.get(0), args.get(1), args.get(2)))
    }
);

impl ProcessSyscallContext<'_> {
    pub(crate) fn madvise(&mut self, _address: u64, _len: u64, _advice: u64) -> SysResult<u64> {
        Err(SysErr::NoSys)
    }
}
