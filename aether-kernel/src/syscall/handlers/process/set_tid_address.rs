use crate::arch::syscall::nr;
use crate::errno::SysResult;
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct SetTidAddressSyscall => nr::SET_TID_ADDRESS, "set_tid_address", |ctx, args| {
        SyscallDisposition::Return(ctx.set_tid_address(args.get(0)))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_set_tid_address(&mut self, address: u64) -> SysResult<u64> {
        self.process.clear_child_tid = (address != 0).then_some(address);
        Ok(self.process.identity.pid as u64)
    }
}
