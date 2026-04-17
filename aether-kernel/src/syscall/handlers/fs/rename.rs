use crate::arch::syscall::nr;
use crate::errno::SysResult;
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(pub struct RenameSyscall => nr::RENAME, "rename", |ctx, args| {
    let Ok(old_path) = ctx.read_user_c_string(args.get(0), 512) else {
        return SyscallDisposition::Return(Err(crate::errno::SysErr::Fault));
    };
    let Ok(new_path) = ctx.read_user_c_string(args.get(1), 512) else {
        return SyscallDisposition::Return(Err(crate::errno::SysErr::Fault));
    };
    SyscallDisposition::Return(ctx.rename(&old_path, &new_path))
});

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_rename(&mut self, old_path: &str, new_path: &str) -> SysResult<u64> {
        self.services.rename(&self.process.fs, old_path, new_path)
    }
}
