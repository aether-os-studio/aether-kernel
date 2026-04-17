use crate::arch::syscall::nr;
use crate::errno::SysResult;
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;
use crate::syscall::abi::read_path;

crate::declare_syscall!(
    pub struct CreatSyscall => nr::CREAT, "creat", |ctx, args| {
        let Ok(path) = read_path(ctx, args.get(0), 512) else {
            return SyscallDisposition::Return(Err(crate::errno::SysErr::Fault));
        };
        SyscallDisposition::Return(ctx.creat(&path, args.get(1)))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_creat(&mut self, path: &str, mode: u64) -> SysResult<u64> {
        const O_CREAT: u64 = 0o100;
        const O_TRUNC: u64 = 0o1000;
        const O_WRONLY: u64 = 0o1;
        self.syscall_openat(-100, path, O_WRONLY | O_CREAT | O_TRUNC, mode)
    }
}
