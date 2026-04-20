use aether_frame::time;

use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;
use crate::syscall::abi::{ITIMER_PROF, ITIMER_REAL, ITIMER_VIRTUAL, LinuxItimerval};

crate::declare_syscall!(
    pub struct GetitimerSyscall => nr::GETITIMER, "getitimer", |ctx, args| {
        SyscallDisposition::Return(ctx.getitimer(args.get(0) as i32, args.get(1)))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_getitimer(&mut self, which: i32, curr_value: u64) -> SysResult<u64> {
        if curr_value == 0 {
            return Err(SysErr::Fault);
        }

        let which = which as u64;
        if which != ITIMER_REAL {
            return match which {
                ITIMER_VIRTUAL | ITIMER_PROF => Err(SysErr::NoSys),
                _ => Err(SysErr::Inval),
            };
        }

        let (remaining_nanos, interval_nanos) = crate::process::read_real_timer(
            self.process.identity.thread_group,
            time::monotonic_nanos(),
        );
        LinuxItimerval::from_nanos(interval_nanos, remaining_nanos).write_to(self, curr_value)?;
        Ok(0)
    }
}
