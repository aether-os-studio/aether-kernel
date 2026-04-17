use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct SetResUidSyscall => nr::SETRESUID, "setresuid", |ctx, args| {
        SyscallDisposition::Return(ctx.setresuid(args.get(0), args.get(1), args.get(2)))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    fn can_set_uid_to(&self, uid: u32) -> bool {
        let cred = &self.process.credentials;
        cred.is_superuser() || uid == cred.uid || uid == cred.euid || uid == cred.suid
    }

    fn read_optional_uid(raw: u64) -> SysResult<Option<u32>> {
        let value = u32::try_from(raw).map_err(|_| SysErr::Inval)?;
        Ok((value != u32::MAX).then_some(value))
    }

    pub(crate) fn syscall_setresuid(&mut self, ruid: u64, euid: u64, suid: u64) -> SysResult<u64> {
        let ruid = Self::read_optional_uid(ruid)?;
        let euid = Self::read_optional_uid(euid)?;
        let suid = Self::read_optional_uid(suid)?;

        if !self.process.credentials.is_superuser() {
            if let Some(uid) = ruid
                && !self.can_set_uid_to(uid)
            {
                return Err(SysErr::Perm);
            }
            if let Some(uid) = euid
                && !self.can_set_uid_to(uid)
            {
                return Err(SysErr::Perm);
            }
            if let Some(uid) = suid
                && !self.can_set_uid_to(uid)
            {
                return Err(SysErr::Perm);
            }
        }

        let credentials = &mut self.process.credentials;
        if let Some(uid) = ruid {
            credentials.uid = uid;
        }
        if let Some(uid) = euid {
            credentials.euid = uid;
            credentials.fsuid = uid;
        }
        if let Some(uid) = suid {
            credentials.suid = uid;
        }
        Ok(0)
    }
}
