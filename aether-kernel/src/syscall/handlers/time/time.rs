use aether_frame::interrupt::timer;

use crate::arch::syscall::nr;
use crate::errno::SysResult;
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::{KernelSyscallContext, SyscallDisposition};

crate::declare_syscall!(pub struct TimeSyscall => nr::TIME, "time", |ctx, args| {
    SyscallDisposition::Return(ctx.time(args.get(0)))
});

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_time(&mut self, tloc: u64) -> SysResult<u64> {
        let (secs, _) = timer::unix_time_nanos();
        if tloc != 0 {
            self.write_user_buffer(tloc, &secs.to_ne_bytes())?;
        }
        Ok(secs as u64)
    }
}
