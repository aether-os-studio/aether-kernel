use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::abi::{
    CLOCK_BOOTTIME, CLOCK_BOOTTIME_ALARM, CLOCK_MONOTONIC, CLOCK_MONOTONIC_COARSE,
    CLOCK_MONOTONIC_RAW, CLOCK_PROCESS_CPUTIME_ID, CLOCK_REALTIME, CLOCK_REALTIME_ALARM,
    CLOCK_REALTIME_COARSE, CLOCK_TAI, CLOCK_THREAD_CPUTIME_ID,
};
use crate::syscall::{KernelSyscallContext, SyscallDisposition};

crate::declare_syscall!(
    pub struct ClockGetresSyscall => nr::CLOCK_GETRES, "clock_getres", |ctx, args| {
        SyscallDisposition::Return(ctx.clock_getres(args.get(0), args.get(1)))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    fn clock_resolution(&self, clock_id: u64) -> SysResult<(i64, i64)> {
        match clock_id {
            CLOCK_REALTIME | CLOCK_MONOTONIC | CLOCK_MONOTONIC_RAW | CLOCK_BOOTTIME
            | CLOCK_REALTIME_ALARM | CLOCK_BOOTTIME_ALARM | CLOCK_TAI => Ok((0, 1)),
            CLOCK_REALTIME_COARSE | CLOCK_MONOTONIC_COARSE => Ok((0, 1_000_000)),
            CLOCK_PROCESS_CPUTIME_ID | CLOCK_THREAD_CPUTIME_ID => Err(SysErr::Inval),
            _ => Err(SysErr::Inval),
        }
    }

    pub(crate) fn syscall_clock_getres(&mut self, clock_id: u64, tp: u64) -> SysResult<u64> {
        if tp == 0 {
            return Ok(0);
        }

        let (secs, nanos) = self.clock_resolution(clock_id)?;
        self.write_user_timespec(tp, secs, nanos)?;
        Ok(0)
    }
}
