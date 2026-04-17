use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct RseqSyscall => nr::RSEQ, "rseq", |ctx, args| {
        SyscallDisposition::Return(ctx.rseq(args.get(0), args.get(1), args.get(2), args.get(3)))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_rseq(
        &mut self,
        _area: u64,
        len: u64,
        flags: u64,
        _signature: u64,
    ) -> SysResult<u64> {
        const RSEQ_AREA_LEN: u64 = 32;

        if flags != 0 || len != RSEQ_AREA_LEN {
            return Err(SysErr::Inval);
        }

        // Proper rseq support needs per-thread registration plus scheduler-assisted CPU-id
        // updates. Until that exists, advertise the feature as unavailable.
        Err(SysErr::NoSys)
    }
}
