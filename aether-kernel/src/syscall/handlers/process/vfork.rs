use crate::arch::syscall::nr;
use crate::process::{CloneParams, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct VforkSyscall => nr::VFORK, "vfork", |ctx, _args| {
        ctx.vfork_blocking()
    }
);

impl ProcessSyscallContext<'_> {
    pub(crate) fn vfork_blocking(&mut self) -> SyscallDisposition {
        self.clone_process_blocking(CloneParams::vfork())
    }
}
