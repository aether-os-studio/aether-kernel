use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext, wait_status};
use crate::syscall::{BlockResult, KernelSyscallContext, SyscallDisposition};

crate::declare_syscall!(
    pub struct Wait4Syscall => nr::WAIT4, "wait4", |ctx, args| {
        ctx.wait4_blocking(args.get(0) as i32, args.get(1), args.get(2), args.get(3))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_wait4(
        &mut self,
        pid: i32,
        status: u64,
        options: u64,
        _rusage: u64,
    ) -> SysResult<u64> {
        if let Some(event) = self
            .services
            .reap_child_event(self.process.identity.pid, pid, options)
        {
            if status != 0 {
                let raw = wait_status(event.kind);
                self.write_user_buffer(status, &raw.to_ne_bytes())?;
            }
            return Ok(event.pid as u64);
        }

        if (options & 1) != 0 {
            if self.services.has_child(self.process.identity.pid, pid) {
                return Ok(0);
            }
            return Err(SysErr::Child);
        }

        if self.services.has_child(self.process.identity.pid, pid) {
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
        self.resumable_blocking_syscall(
            |_ctx, result| match result {
                BlockResult::CompletedValue { value } => SyscallDisposition::ok(value),
                BlockResult::SignalInterrupted => SyscallDisposition::err(SysErr::Intr),
                _ => SyscallDisposition::err(SysErr::Intr),
            },
            |ctx| ctx.syscall_wait4(pid, status, options, rusage),
            |ctx| ctx.block_wait_child(pid, status, options),
        )
    }
}
