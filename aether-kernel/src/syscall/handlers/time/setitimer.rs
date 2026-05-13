use aether_frame::time;

use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::ProcessSyscallContext;
use crate::syscall::SyscallDisposition;
use crate::syscall::abi::{ITIMER_REAL, LinuxItimerval};

crate::declare_syscall!(
    pub struct SetitimerSyscall => nr::SETITIMER, "setitimer", |ctx, args| {
        SyscallDisposition::Return(ctx.setitimer(args.get(0) as i32, args.get(1), args.get(2)))
    }
);

impl ProcessSyscallContext<'_> {
    pub(crate) fn setitimer(
        &mut self,
        which: i32,
        new_value: u64,
        old_value: u64,
    ) -> SysResult<u64> {
        if which as u64 != ITIMER_REAL {
            return Err(SysErr::NoSys);
        }

        let now_nanos = time::monotonic_nanos();
        let tgid = self.process.identity.thread_group;
        let (old_remaining_nanos, old_interval_nanos) =
            crate::process::read_real_timer(tgid, now_nanos);

        if old_value != 0 {
            LinuxItimerval::from_nanos(old_interval_nanos, old_remaining_nanos)
                .write_to(self, old_value)?;
        }

        if new_value != 0 {
            let new_timer = LinuxItimerval::read_from(self, new_value)?;
            crate::process::set_real_timer(
                tgid,
                now_nanos,
                new_timer.it_value.total_nanos()?,
                new_timer.it_interval.total_nanos()?,
            );
        }

        Ok(0)
    }
}
