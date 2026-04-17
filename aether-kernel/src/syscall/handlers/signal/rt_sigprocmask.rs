use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext, decode_sigset};
use crate::signal::{SIG_BLOCK, SIG_SETMASK, SIG_UNBLOCK};
use crate::syscall::{KernelSyscallContext, SyscallDisposition};

crate::declare_syscall!(
    pub struct RtSigprocmaskSyscall => nr::RT_SIGPROCMASK, "rt_sigprocmask", |ctx, args| {
        SyscallDisposition::Return(ctx.rt_sigprocmask(args.get(0), args.get(1), args.get(2), args.get(3)))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_rt_sigprocmask(
        &mut self,
        how: u64,
        set: u64,
        oldset: u64,
        sigsetsize: u64,
    ) -> SysResult<u64> {
        if sigsetsize < 8 {
            return Err(SysErr::Inval);
        }

        if oldset != 0 {
            let bytes = self.process.signals.blocked().to_ne_bytes();
            self.write_user_buffer(oldset, &bytes[..sigsetsize as usize])?;
        }

        if set != 0 {
            let raw = self.read_user_buffer(set, sigsetsize as usize)?;
            let mask = decode_sigset(&raw);
            match how {
                SIG_BLOCK | SIG_UNBLOCK | SIG_SETMASK => self.process.signals.set_mask(how, mask),
                _ => return Err(SysErr::Inval),
            }
        }
        Ok(0)
    }
}
