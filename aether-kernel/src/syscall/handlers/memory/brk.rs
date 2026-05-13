use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::ProcessSyscallContext;
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct BrkSyscall => nr::BRK, "brk", |ctx, args| { SyscallDisposition::Return(ctx.brk(args.get(0))) }
);

impl ProcessSyscallContext<'_> {
    pub(crate) fn brk(&mut self, address: u64) -> SysResult<u64> {
        self.process
            .task
            .address_space
            .brk(address)
            .map_err(SysErr::from)
    }
}
