use crate::arch::syscall::nr;
use crate::errno::SysResult;
use crate::fs::{make_stat, serialize_stat};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::KernelSyscallContext;
use crate::syscall::SyscallDisposition;
use crate::syscall::abi::read_path;

crate::declare_syscall!(
    pub struct StatSyscall => nr::STAT, "stat", |ctx, args| {
        let Ok(path) = read_path(ctx, args.get(0), 512) else {
            return SyscallDisposition::Return(Err(crate::errno::SysErr::Fault));
        };
        SyscallDisposition::Return(ctx.stat_path(&path, args.get(1)))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_stat_path(&mut self, path: &str, address: u64) -> SysResult<u64> {
        let (node, _) = self
            .services
            .lookup_node_with_identity(&self.process.fs, path, true)?;
        let stat = make_stat(&node);
        let bytes = serialize_stat(&stat);
        self.write_user_buffer(address, &bytes)?;
        Ok(0)
    }
}
