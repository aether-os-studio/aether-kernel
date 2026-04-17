use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct CloseRangeSyscall => nr::CLOSE_RANGE, "close_range", |ctx, args| {
        SyscallDisposition::Return(ctx.close_range(args.get(0), args.get(1), args.get(2)))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_close_range(
        &mut self,
        first: u64,
        last: u64,
        flags: u64,
    ) -> SysResult<u64> {
        const CLOSE_RANGE_CLOEXEC: u64 = 0x4;
        const CLOSE_RANGE_UNSHARE: u64 = 0x2;

        if first > last || (flags & !(CLOSE_RANGE_CLOEXEC | CLOSE_RANGE_UNSHARE)) != 0 {
            return Err(SysErr::Inval);
        }
        if first > u32::MAX as u64 {
            return Ok(0);
        }

        let end = core::cmp::min(last, u32::MAX as u64) as u32;
        let start = first as u32;
        if (flags & CLOSE_RANGE_CLOEXEC) != 0 {
            self.process.files.set_cloexec_range(start, end);
        } else {
            // The kernel does not currently share file descriptor tables across tasks, so
            // CLOSE_RANGE_UNSHARE is satisfied by applying the operation to this process table.
            self.process.files.close_range(start, end);
        }

        Ok(0)
    }
}
