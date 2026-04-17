use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct SetGidSyscall => nr::SETGID, "setgid", |ctx, args| {
        SyscallDisposition::Return(ctx.setgid(args.get(0)))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_setgid(&mut self, gid: u64) -> SysResult<u64> {
        let gid = u32::try_from(gid).map_err(|_| SysErr::Inval)?;
        let credentials = &mut self.process.credentials;

        if gid == u32::MAX {
            return Err(SysErr::Inval);
        }
        if credentials.is_superuser() {
            credentials.gid = gid;
            credentials.egid = gid;
            credentials.sgid = gid;
            credentials.fsgid = gid;
            return Ok(0);
        }

        if gid != credentials.gid && gid != credentials.egid && gid != credentials.sgid {
            return Err(SysErr::Perm);
        }

        credentials.egid = gid;
        credentials.fsgid = gid;
        Ok(0)
    }
}
