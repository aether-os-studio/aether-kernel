use aether_frame::interrupt::timer;

use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::abi::{
    CLOCK_REALTIME, LinuxTimespec, TIMER_ABSTIME, validate_clock_nanosleep_clock,
};
use crate::syscall::{BlockResult, KernelSyscallContext, SyscallDisposition};

crate::declare_syscall!(
    pub struct ClockNanosleepSyscall => nr::CLOCK_NANOSLEEP, "clock_nanosleep", |ctx, args| {
        ctx.clock_nanosleep_blocking(
            args.get(0),
            args.get(1),
            args.get(2),
            args.get(3),
        )
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_clock_nanosleep(
        &mut self,
        clock_id: u64,
        flags: u64,
        rqtp: u64,
        _rmtp: u64,
    ) -> SysResult<u64> {
        if let Some(result) = self.process.wake_result.take() {
            return match result {
                BlockResult::Timer {
                    completed,
                    remaining_nanos,
                    rmtp,
                    is_absolute,
                } => {
                    if completed {
                        Ok(0)
                    } else {
                        if !is_absolute && rmtp != 0 {
                            self.write_user_timespec(
                                rmtp,
                                (remaining_nanos / 1_000_000_000) as i64,
                                (remaining_nanos % 1_000_000_000) as i64,
                            )?;
                        }
                        Err(SysErr::Intr)
                    }
                }
                BlockResult::SignalInterrupted => Err(SysErr::Intr),
                _ => Err(SysErr::Intr),
            };
        }

        validate_clock_nanosleep_clock(clock_id)?;

        if rqtp == 0 {
            return Err(SysErr::Inval);
        }

        let request = LinuxTimespec::read_from(self, rqtp)?.validate()?;
        let current_nanos = timer::nanos_since_boot();
        let request_nanos = request.total_nanos()?;

        let target_nanos = if (flags & TIMER_ABSTIME) != 0 {
            if clock_id == CLOCK_REALTIME {
                let boot_time = aether_frame::boot::info().boot_time.unwrap_or(0);
                let current_unix_nanos = boot_time as u64 * 1_000_000_000 + current_nanos;
                if request_nanos <= current_unix_nanos {
                    return Ok(0);
                }
                request_nanos.saturating_sub(boot_time as u64 * 1_000_000_000)
            } else {
                if request_nanos <= current_nanos {
                    return Ok(0);
                }
                request_nanos
            }
        } else {
            if request_nanos == 0 {
                return Ok(0);
            }
            current_nanos.saturating_add(request_nanos)
        };

        if target_nanos <= current_nanos {
            return Ok(0);
        }

        Err(SysErr::Again)
    }

    pub(crate) fn syscall_clock_nanosleep_blocking(
        &mut self,
        clock_id: u64,
        flags: u64,
        rqtp: u64,
        rmtp: u64,
    ) -> SyscallDisposition {
        if let Err(error) = validate_clock_nanosleep_clock(clock_id) {
            return SyscallDisposition::err(error);
        }
        if rqtp == 0 {
            return SyscallDisposition::err(SysErr::Inval);
        }
        let request = match LinuxTimespec::read_from(self, rqtp).and_then(LinuxTimespec::validate) {
            Ok(request) => request,
            Err(error) => return SyscallDisposition::err(error),
        };
        let current_nanos = timer::nanos_since_boot();
        let request_nanos = match request.total_nanos() {
            Ok(value) => value,
            Err(error) => return SyscallDisposition::err(error),
        };

        let target_nanos = if (flags & TIMER_ABSTIME) != 0 {
            if clock_id == CLOCK_REALTIME {
                let boot_time = aether_frame::boot::info().boot_time.unwrap_or(0);
                let current_unix_nanos = boot_time as u64 * 1_000_000_000 + current_nanos;
                if request_nanos <= current_unix_nanos {
                    return SyscallDisposition::ok(0);
                }
                request_nanos.saturating_sub(boot_time as u64 * 1_000_000_000)
            } else {
                if request_nanos <= current_nanos {
                    return SyscallDisposition::ok(0);
                }
                request_nanos
            }
        } else {
            if request_nanos == 0 {
                return SyscallDisposition::ok(0);
            }
            current_nanos.saturating_add(request_nanos)
        };

        self.resumable_blocking_syscall(
            |ctx, result| match result {
                BlockResult::Timer {
                    completed,
                    remaining_nanos,
                    rmtp,
                    is_absolute,
                } => {
                    if completed {
                        SyscallDisposition::ok(0)
                    } else {
                        if !is_absolute && rmtp != 0 {
                            if let Err(error) = ctx.write_user_timespec(
                                rmtp,
                                (remaining_nanos / 1_000_000_000) as i64,
                                (remaining_nanos % 1_000_000_000) as i64,
                            ) {
                                return SyscallDisposition::err(error);
                            }
                        }
                        SyscallDisposition::err(SysErr::Intr)
                    }
                }
                BlockResult::SignalInterrupted => SyscallDisposition::err(SysErr::Intr),
                _ => SyscallDisposition::err(SysErr::Intr),
            },
            |ctx| ctx.syscall_clock_nanosleep(clock_id, flags, rqtp, rmtp),
            |ctx| ctx.block_timer(target_nanos, request_nanos, rmtp, flags),
        )
    }
}
