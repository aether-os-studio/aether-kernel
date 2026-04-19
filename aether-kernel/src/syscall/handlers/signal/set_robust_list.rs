use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct SetRobustListSyscall => nr::SET_ROBUST_LIST, "set_robust_list", |ctx, args| {
        SyscallDisposition::Return(ctx.set_robust_list(args.get(0), args.get(1)))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_set_robust_list(&mut self, head: u64, len: u64) -> SysResult<u64> {
        const ROBUST_LIST_HEAD_LEN: u64 = 24;

        if len != ROBUST_LIST_HEAD_LEN {
            return Err(SysErr::Inval);
        }
        self.process.robust_list_head = (head != 0).then_some(head);
        self.process.robust_list_len = len;
        Ok(0)
    }
}
