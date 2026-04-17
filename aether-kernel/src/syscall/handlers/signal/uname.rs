use crate::arch::syscall::nr;
use crate::errno::SysResult;
use crate::fs::{LinuxUtsName, serialize_utsname};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::{KernelSyscallContext, SyscallDisposition};

crate::declare_syscall!(
    pub struct UnameSyscall => nr::UNAME, "uname", |ctx, args| {
        SyscallDisposition::Return(ctx.uname(args.get(0)))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_uname(&mut self, address: u64) -> SysResult<u64> {
        let uts = LinuxUtsName::linux_x86_64();
        let bytes = serialize_utsname(&uts);
        self.write_user_buffer(address, &bytes)?;
        Ok(0)
    }
}
