use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::KernelSyscallContext;
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct GetResUidSyscall => nr::GETRESUID, "getresuid", |ctx, args| {
        SyscallDisposition::Return(ctx.getresuid(args.get(0), args.get(1), args.get(2)))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_getresuid(&mut self, ruid: u64, euid: u64, suid: u64) -> SysResult<u64> {
        if ruid == 0 || euid == 0 || suid == 0 {
            return Err(SysErr::Fault);
        }

        self.write_user_buffer(ruid, &self.process.credentials.uid.to_ne_bytes())?;
        self.write_user_buffer(euid, &self.process.credentials.euid.to_ne_bytes())?;
        self.write_user_buffer(suid, &self.process.credentials.suid.to_ne_bytes())?;
        Ok(0)
    }
}
