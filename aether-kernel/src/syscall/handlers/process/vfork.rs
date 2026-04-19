use crate::arch::syscall::nr;
use crate::process::{CloneParams, ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct VforkSyscall => nr::VFORK, "vfork", |ctx, _args| {
        ctx.vfork_blocking()
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_vfork_blocking(&mut self) -> SyscallDisposition {
        self.syscall_clone_process_blocking(CloneParams::vfork())
    }
}
