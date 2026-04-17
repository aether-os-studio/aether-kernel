use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct SetResGidSyscall => nr::SETRESGID, "setresgid", |ctx, args| {
        SyscallDisposition::Return(ctx.setresgid(args.get(0), args.get(1), args.get(2)))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    fn can_set_gid_to(&self, gid: u32) -> bool {
        let cred = &self.process.credentials;
        cred.is_superuser() || gid == cred.gid || gid == cred.egid || gid == cred.sgid
    }

    fn read_optional_gid(raw: u64) -> SysResult<Option<u32>> {
        let value = u32::try_from(raw).map_err(|_| SysErr::Inval)?;
        Ok((value != u32::MAX).then_some(value))
    }

    pub(crate) fn syscall_setresgid(&mut self, rgid: u64, egid: u64, sgid: u64) -> SysResult<u64> {
        let rgid = Self::read_optional_gid(rgid)?;
        let egid = Self::read_optional_gid(egid)?;
        let sgid = Self::read_optional_gid(sgid)?;

        if !self.process.credentials.is_superuser() {
            if let Some(gid) = rgid
                && !self.can_set_gid_to(gid)
            {
                return Err(SysErr::Perm);
            }
            if let Some(gid) = egid
                && !self.can_set_gid_to(gid)
            {
                return Err(SysErr::Perm);
            }
            if let Some(gid) = sgid
                && !self.can_set_gid_to(gid)
            {
                return Err(SysErr::Perm);
            }
        }

        let credentials = &mut self.process.credentials;
        if let Some(gid) = rgid {
            credentials.gid = gid;
        }
        if let Some(gid) = egid {
            credentials.egid = gid;
            credentials.fsgid = gid;
        }
        if let Some(gid) = sgid {
            credentials.sgid = gid;
        }
        Ok(0)
    }
}
