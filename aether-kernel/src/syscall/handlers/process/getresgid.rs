use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::KernelSyscallContext;
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct GetResGidSyscall => nr::GETRESGID, "getresgid", |ctx, args| {
        SyscallDisposition::Return(ctx.getresgid(args.get(0), args.get(1), args.get(2)))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_getresgid(&mut self, rgid: u64, egid: u64, sgid: u64) -> SysResult<u64> {
        if rgid == 0 || egid == 0 || sgid == 0 {
            return Err(SysErr::Fault);
        }

        self.write_user_buffer(rgid, &self.process.credentials.gid.to_ne_bytes())?;
        self.write_user_buffer(egid, &self.process.credentials.egid.to_ne_bytes())?;
        self.write_user_buffer(sgid, &self.process.credentials.sgid.to_ne_bytes())?;
        Ok(0)
    }
}
