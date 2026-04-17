use crate::arch::syscall::nr;
use crate::errno::SysResult;
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;
use crate::syscall::abi::arg_i64_from_i32;

crate::declare_syscall!(
    pub struct UnlinkAtSyscall => nr::UNLINKAT, "unlinkat", |ctx, args| {
        let Ok(path) = ctx.read_user_c_string(args.get(1), 512) else {
            return SyscallDisposition::Return(Err(crate::errno::SysErr::Fault));
        };
        SyscallDisposition::Return(ctx.unlinkat(arg_i64_from_i32(args.get(0)), &path, args.get(2)))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_unlinkat(
        &mut self,
        dirfd: i64,
        path: &str,
        flags: u64,
    ) -> SysResult<u64> {
        let fs_view = self.fs_view_for_dirfd(dirfd, path)?;
        self.services.unlink(&fs_view, path, flags)
    }
}
