use aether_frame::time;

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
    pub struct ClockGettimeSyscall => nr::CLOCK_GETTIME, "clock_gettime", |ctx, args| {
        SyscallDisposition::Return(ctx.clock_gettime(args.get(0), args.get(1)))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    fn read_clock_timespec(&self, clock_id: u64) -> SysResult<(i64, i64)> {
        let monotonic_nanos = time::monotonic_nanos();
        let monotonic_secs = (monotonic_nanos / 1_000_000_000) as i64;
        let monotonic_subsec = (monotonic_nanos % 1_000_000_000) as i64;

        let (realtime_secs, realtime_subsec) = time::realtime_nanos();

        match clock_id {
            CLOCK_REALTIME | CLOCK_REALTIME_COARSE | CLOCK_REALTIME_ALARM => {
                Ok((realtime_secs, realtime_subsec as i64))
            }
            CLOCK_MONOTONIC
            | CLOCK_MONOTONIC_RAW
            | CLOCK_MONOTONIC_COARSE
            | CLOCK_BOOTTIME
            | CLOCK_BOOTTIME_ALARM => Ok((monotonic_secs, monotonic_subsec)),
            CLOCK_TAI => {
                // TODO: Linux CLOCK_TAI should include the current TAI-UTC offset.
                Ok((realtime_secs, realtime_subsec as i64))
            }
            CLOCK_PROCESS_CPUTIME_ID | CLOCK_THREAD_CPUTIME_ID => Err(SysErr::Inval),
            _ => Err(SysErr::Inval),
        }
    }

    pub(crate) fn syscall_clock_gettime(&mut self, clock_id: u64, tp: u64) -> SysResult<u64> {
        if tp == 0 {
            return Err(SysErr::Fault);
        }

        let (secs, nanos) = self.read_clock_timespec(clock_id)?;
        self.write_user_timespec(tp, secs, nanos)?;
        Ok(0)
    }
}
