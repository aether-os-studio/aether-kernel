use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct SetUidSyscall => nr::SETUID, "setuid", |ctx, args| {
        SyscallDisposition::Return(ctx.setuid(args.get(0)))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_setuid(&mut self, uid: u64) -> SysResult<u64> {
        let uid = u32::try_from(uid).map_err(|_| SysErr::Inval)?;
        if uid == u32::MAX {
            return Err(SysErr::Inval);
        }

        let credentials = &mut self.process.credentials;
        if credentials.is_superuser() {
            credentials.uid = uid;
            credentials.euid = uid;
            credentials.suid = uid;
            credentials.fsuid = uid;
            return Ok(0);
        }

        if uid != credentials.uid && uid != credentials.euid && uid != credentials.suid {
            return Err(SysErr::Perm);
        }

        credentials.euid = uid;
        credentials.fsuid = uid;
        Ok(0)
    }
}
