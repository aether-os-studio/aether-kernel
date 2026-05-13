use crate::arch::syscall::nr;
use crate::errno::SysResult;
use crate::fs::{LinuxUtsName, serialize_utsname};
use crate::process::ProcessSyscallContext;
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct UnameSyscall => nr::UNAME, "uname", |ctx, args| {
        SyscallDisposition::Return(ctx.uname(args.get(0)))
    }
);

impl ProcessSyscallContext<'_> {
    pub(crate) fn uname(&mut self, address: u64) -> SysResult<u64> {
        let uts = LinuxUtsName::linux_x86_64();
        let bytes = serialize_utsname(&uts);
        self.write_user_buffer(address, &bytes)?;
        Ok(0)
    }
}
