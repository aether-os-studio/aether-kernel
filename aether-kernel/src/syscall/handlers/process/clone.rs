use crate::arch::syscall::nr;
use crate::errno::SysResult;
use crate::process::{CloneParams, ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct CloneSyscall => nr::CLONE, "clone", |ctx, args| {
        let params = CloneParams::from_clone(
            args.get(0),
            args.get(1),
            args.get(2),
            args.get(3),
            args.get(4),
        );
        SyscallDisposition::Return(ctx.clone_process(params))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_clone_process(&mut self, params: CloneParams) -> SysResult<u64> {
        let pid = self.services.clone_process(self.process, params)? as u64;
        Ok(pid)
    }
}
