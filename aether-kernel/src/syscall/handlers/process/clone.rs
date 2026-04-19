use crate::arch::syscall::nr;
use crate::errno::SysResult;
use crate::process::{CloneParams, ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct CloneSyscall => nr::CLONE, "clone", |ctx, args| {
        let params = CloneParams::from_clone(
            args.get(0),
            args.get(1),
            args.get(2),
            args.get(3),
            args.get(4),
        );
        ctx.clone_process_blocking(params)
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_clone_process(&mut self, params: CloneParams) -> SysResult<u64> {
        let pid = self.services.clone_process(self.process, params)? as u64;
        Ok(pid)
    }

    pub(crate) fn syscall_clone_process_blocking(
        &mut self,
        params: CloneParams,
    ) -> SyscallDisposition {
        if params.is_vfork()
            && let Some(result) = self.process.wake_result.take()
        {
            return match result {
                crate::syscall::BlockResult::CompletedValue { value } => {
                    SyscallDisposition::ok(value)
                }
                crate::syscall::BlockResult::SignalInterrupted => {
                    SyscallDisposition::err(crate::errno::SysErr::Intr)
                }
                _ => SyscallDisposition::err(crate::errno::SysErr::Intr),
            };
        }

        match self.services.clone_process(self.process, params) {
            Ok(child) if params.is_vfork() => match self.wait_vfork(child) {
                Ok(crate::syscall::BlockResult::CompletedValue { value }) => {
                    SyscallDisposition::ok(value)
                }
                Ok(crate::syscall::BlockResult::SignalInterrupted) => {
                    SyscallDisposition::err(crate::errno::SysErr::Intr)
                }
                Ok(_) => SyscallDisposition::err(crate::errno::SysErr::Intr),
                Err(disposition) => disposition,
            },
            Ok(child) => SyscallDisposition::ok(child as u64),
            Err(error) => SyscallDisposition::err(error),
        }
    }
}
