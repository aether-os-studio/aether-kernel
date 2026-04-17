use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(pub struct FchownSyscall => nr::FCHOWN, "fchown", |ctx, args| {
    SyscallDisposition::Return(ctx.fchown(args.get(0), args.get(1), args.get(2)))
});

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_fchown(&mut self, fd: u64, owner: u64, group: u64) -> SysResult<u64> {
        let owner = Self::read_optional_chown_id(owner)?;
        let group = Self::read_optional_chown_id(group)?;

        let descriptor = self.process.files.get(fd as u32).ok_or(SysErr::BadFd)?;
        let node = descriptor.file.lock().node();
        let metadata = node.metadata();
        self.may_chown(metadata.uid, metadata.gid, owner, group)?;

        let next_uid = owner.unwrap_or(metadata.uid);
        let next_gid = group.unwrap_or(metadata.gid);
        if next_uid == metadata.uid && next_gid == metadata.gid {
            return Ok(0);
        }

        node.set_owner(next_uid, next_gid).map_err(SysErr::from)?;
        crate::fs::notify_attrib(&node);
        Ok(0)
    }
}
