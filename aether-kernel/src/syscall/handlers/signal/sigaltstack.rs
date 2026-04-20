use core::mem::size_of;

use crate::arch::syscall::nr;
use crate::errno::SysResult;
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::signal::{SignalStack, parse_signal_stack, serialize_signal_stack};
use crate::syscall::{KernelSyscallContext, SyscallDisposition};

crate::declare_syscall!(
    pub struct SigaltstackSyscall => nr::SIGALTSTACK, "sigaltstack", |ctx, args| {
        SyscallDisposition::Return(ctx.sigaltstack(args.get(0), args.get(1)))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_sigaltstack(&mut self, uss: u64, uoss: u64) -> SysResult<u64> {
        let new_stack = if uss == 0 {
            None
        } else {
            let raw = self.syscall_read_user_exact_buffer(uss, size_of::<SignalStack>())?;
            Some(parse_signal_stack(&raw).ok_or(crate::errno::SysErr::Fault)?)
        };

        let user_sp = self.process.task.process.context().general.rsp;
        let old_stack = self.process.signals.set_altstack(new_stack, user_sp)?;

        if uoss != 0 {
            let raw = serialize_signal_stack(old_stack);
            self.write_user_buffer(uoss, &raw)?;
        }

        Ok(0)
    }
}
