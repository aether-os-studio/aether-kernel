use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{
    ProcessServices, ProcessSyscallContext, WaitChildApi, WaitChildSelector, wait_status,
};
use crate::syscall::{BlockResult, KernelSyscallContext, SyscallDisposition};

crate::declare_syscall!(
    pub struct Wait4Syscall => nr::WAIT4, "wait4", |ctx, args| {
        ctx.wait4_blocking(args.get(0) as i32, args.get(1), args.get(2), args.get(3))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    fn wait4_selector(&self, pid: i32) -> SysResult<WaitChildSelector> {
        Ok(match pid {
            -1 => WaitChildSelector::Any,
            0 => WaitChildSelector::ProcessGroup(self.process.identity.process_group),
            value if value < -1 => WaitChildSelector::ProcessGroup(value.unsigned_abs()),
            value => WaitChildSelector::Pid(value as u32),
        })
    }

    pub(crate) fn syscall_wait4(
        &mut self,
        pid: i32,
        status: u64,
        options: u64,
        _rusage: u64,
    ) -> SysResult<u64> {
        let selector = self.wait4_selector(pid)?;
        if let Some(event) =
            self.services
                .wait_child_event(self.process.identity.pid, selector, options, true)
        {
            if status != 0 {
                let raw = wait_status(event.kind);
                self.write_user_buffer(status, &raw.to_ne_bytes())?;
            }
            return Ok(event.pid as u64);
        }

        if (options & 1) != 0 {
            if self
                .services
                .has_waitable_child(self.process.identity.pid, selector)
            {
                return Ok(0);
            }
            return Err(SysErr::Child);
        }

        if self
            .services
            .has_waitable_child(self.process.identity.pid, selector)
        {
            return Err(SysErr::Again);
        }
        Err(SysErr::Child)
    }

    pub(crate) fn syscall_wait4_blocking(
        &mut self,
        pid: i32,
        status: u64,
        options: u64,
        rusage: u64,
    ) -> SyscallDisposition {
        let selector = match self.wait4_selector(pid) {
            Ok(selector) => selector,
            Err(error) => return SyscallDisposition::err(error),
        };
        let _ = rusage;
        if let Some(result) = self.process.wake_result.take() {
            return match result {
                BlockResult::CompletedValue { value } => SyscallDisposition::ok(value),
                BlockResult::SignalInterrupted => SyscallDisposition::err(SysErr::Intr),
                _ => SyscallDisposition::err(SysErr::Intr),
            };
        }

        match self.syscall_wait4(pid, status, options, rusage) {
            Ok(value) => {
                // TODO: populate `rusage` with real child resource usage once the kernel tracks it.
                SyscallDisposition::ok(value)
            }
            Err(SysErr::Again) => {
                match self.wait_wait_child(selector, WaitChildApi::Wait4, status, 0, options) {
                    Ok(BlockResult::CompletedValue { value }) => SyscallDisposition::ok(value),
                    Ok(BlockResult::SignalInterrupted) => SyscallDisposition::err(SysErr::Intr),
                    Ok(_) => SyscallDisposition::err(SysErr::Intr),
                    Err(disposition) => disposition,
                }
            }
            Err(error) => SyscallDisposition::err(error),
        }
    }
}
