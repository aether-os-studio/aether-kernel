use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::ProcessSyscallContext;
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct SetSidSyscall => nr::SETSID, "setsid", |ctx, _args| {
        SyscallDisposition::Return(ctx.setsid())
    }
);

impl ProcessSyscallContext<'_> {
    pub(crate) fn setsid(&mut self) -> SysResult<u64> {
        if self.process.identity.process_group == self.process.identity.pid {
            return Err(SysErr::Perm);
        }

        let pid = self.process.identity.pid;
        self.process.identity.process_group = pid;
        self.process.identity.session = pid;
        // TODO: Linux also detaches the old controlling terminal from the full
        // session. We currently clear only the caller's inherited association.
        self.process.controlling_terminal = None;
        Ok(pid as u64)
    }
}
