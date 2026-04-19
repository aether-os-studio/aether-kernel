use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct SetSidSyscall => nr::SETSID, "setsid", |ctx, _args| {
        SyscallDisposition::Return(ctx.setsid())
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_setsid(&mut self) -> SysResult<u64> {
        if self.process.identity.process_group == self.process.identity.pid {
            return Err(SysErr::Perm);
        }

        let pid = self.process.identity.pid;
        self.process.identity.process_group = pid;
        self.process.identity.session = pid;
        Ok(pid as u64)
    }
}
