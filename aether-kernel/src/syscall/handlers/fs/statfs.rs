use crate::arch::syscall::nr;
use crate::errno::SysResult;
use crate::fs::serialize_statfs;
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::KernelSyscallContext;
use crate::syscall::SyscallDisposition;
use crate::syscall::abi::read_path;

crate::declare_syscall!(
    pub struct StatfsSyscall => nr::STATFS, "statfs", |ctx, args| {
        let Ok(path) = read_path(ctx, args.get(0), 512) else {
            return SyscallDisposition::Return(Err(crate::errno::SysErr::Fault));
        };
        SyscallDisposition::Return(ctx.statfs_path(&path, args.get(1)))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_statfs_path(&mut self, path: &str, address: u64) -> SysResult<u64> {
        let statfs = self.services.statfs(&self.process.fs, path)?;
        let bytes = serialize_statfs(&statfs);
        self.write_user_buffer(address, &bytes)?;
        Ok(0)
    }
}
