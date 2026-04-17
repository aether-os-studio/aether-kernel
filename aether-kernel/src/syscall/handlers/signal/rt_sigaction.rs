use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::signal::{parse_sigaction, serialize_sigaction};
use crate::syscall::{KernelSyscallContext, SyscallDisposition};

crate::declare_syscall!(
    pub struct RtSigactionSyscall => nr::RT_SIGACTION, "rt_sigaction", |ctx, args| {
        SyscallDisposition::Return(ctx.rt_sigaction(args.get(0), args.get(1), args.get(2), args.get(3)))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_rt_sigaction(
        &mut self,
        signal: u64,
        act: u64,
        oldact: u64,
        sigsetsize: u64,
    ) -> SysResult<u64> {
        if sigsetsize < 8 {
            return Err(SysErr::Inval);
        }

        let signal = signal as u8;
        if let Some(action) = self.process.signals.action(signal)
            && oldact != 0
        {
            let bytes = serialize_sigaction(action);
            self.write_user_buffer(oldact, &bytes)?;
        }

        if act != 0 {
            let raw = self.syscall_read_user_exact_buffer(act, 32)?;
            let action = parse_sigaction(&raw).ok_or(SysErr::Inval)?;
            if !self.process.signals.set_action(signal, action) {
                return Err(SysErr::Inval);
            }
        }

        Ok(0)
    }
}
