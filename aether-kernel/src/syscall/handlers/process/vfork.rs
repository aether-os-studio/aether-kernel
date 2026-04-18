use crate::arch::syscall::nr;
use crate::errno::SysErr;
use crate::process::{CloneParams, ProcessServices, ProcessSyscallContext};
use crate::syscall::{BlockResult, SyscallDisposition};

crate::declare_syscall!(
    pub struct VforkSyscall => nr::VFORK, "vfork", |ctx, _args| {
        ctx.vfork_blocking()
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_vfork_blocking(&mut self) -> SyscallDisposition {
        if let Some(result) = self.process.wake_result.take() {
            return match result {
                BlockResult::CompletedValue { value } => SyscallDisposition::ok(value),
                BlockResult::SignalInterrupted => SyscallDisposition::err(SysErr::Intr),
                _ => SyscallDisposition::err(SysErr::Intr),
            };
        }

        match self
            .services
            .clone_process(self.process, CloneParams::vfork())
        {
            Ok(child) => match self.wait_vfork(child) {
                Ok(BlockResult::CompletedValue { value }) => SyscallDisposition::ok(value),
                Ok(BlockResult::SignalInterrupted) => SyscallDisposition::err(SysErr::Intr),
                Ok(_) => SyscallDisposition::err(SysErr::Intr),
                Err(disposition) => disposition,
            },
            Err(error) => SyscallDisposition::err(error),
        }
    }
}
