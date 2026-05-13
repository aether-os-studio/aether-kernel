use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::ProcessSyscallContext;
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct RseqSyscall => nr::RSEQ, "rseq", |ctx, args| {
        SyscallDisposition::Return(ctx.rseq(args.get(0), args.get(1), args.get(2), args.get(3)))
    }
);

impl ProcessSyscallContext<'_> {
    pub(crate) fn rseq(
        &mut self,
        _area: u64,
        len: u64,
        flags: u64,
        _signature: u64,
    ) -> SysResult<u64> {
        if len != crate::process::KernelProcess::RSEQ_AREA_LEN {
            return Err(SysErr::Inval);
        }

        if flags != 0 {
            return Err(SysErr::Inval);
        }

        Err(SysErr::NoSys)
    }
}
