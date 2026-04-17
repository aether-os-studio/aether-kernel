use crate::arch::syscall::nr;
use crate::errno::SysErr;
use crate::errno::SysResult;
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;
use crate::syscall::abi::read_path;

crate::declare_syscall!(
    pub struct PivotRootSyscall => nr::PIVOT_ROOT, "pivot_root", |ctx, args| {
        let Ok(new_root) = read_path(ctx, args.get(0), 256) else {
            return SyscallDisposition::Return(Err(SysErr::Fault));
        };
        let Ok(put_old) = read_path(ctx, args.get(1), 256) else {
            return SyscallDisposition::Return(Err(SysErr::Fault));
        };

        SyscallDisposition::Return(ctx.pivot_root(&new_root, &put_old))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_pivot_root(&mut self, new_root: &str, put_old: &str) -> SysResult<u64> {
        self.services
            .pivot_root(&mut self.process.fs, new_root, put_old)
    }
}
