use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext, decode_sigset};
use crate::syscall::{KernelSyscallContext, SyscallDisposition};

crate::declare_syscall!(
    pub struct RtSigsuspendSyscall => nr::RT_SIGSUSPEND, "rt_sigsuspend", |ctx, args| {
        ctx.rt_sigsuspend_blocking(args.get(0), args.get(1))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_rt_sigsuspend(&mut self, mask: u64, sigsetsize: u64) -> SysResult<u64> {
        if sigsetsize < 8 {
            return Err(SysErr::Inval);
        }
        if mask != 0 {
            let raw = self.read_user_buffer(mask, sigsetsize as usize)?;
            self.process.signals.enter_sigsuspend(decode_sigset(&raw));
        }
        if self
            .process
            .signals
            .has_deliverable(crate::arch::supports_user_handlers())
        {
            self.process.signals.leave_sigsuspend();
            return Err(SysErr::Intr);
        }
        Err(SysErr::Again)
    }

    pub(crate) fn syscall_rt_sigsuspend_blocking(
        &mut self,
        mask: u64,
        sigsetsize: u64,
    ) -> SyscallDisposition {
        match self.syscall_rt_sigsuspend(mask, sigsetsize) {
            Err(SysErr::Again) => {}
            Ok(value) => return SyscallDisposition::ok(value),
            Err(error) => return SyscallDisposition::err(error),
        }

        match self.wait_signal_suspend() {
            Ok(crate::syscall::BlockResult::SignalInterrupted) => {
                SyscallDisposition::err(SysErr::Intr)
            }
            Ok(_) => SyscallDisposition::err(SysErr::Intr),
            Err(disposition) => disposition,
        }
    }
}
