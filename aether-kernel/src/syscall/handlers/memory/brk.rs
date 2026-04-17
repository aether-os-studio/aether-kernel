use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct BrkSyscall => nr::BRK, "brk", |ctx, args| { SyscallDisposition::Return(ctx.brk(args.get(0))) }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_brk(&mut self, address: u64) -> SysResult<u64> {
        self.process
            .task
            .address_space
            .brk(address)
            .map_err(SysErr::from)
    }
}
