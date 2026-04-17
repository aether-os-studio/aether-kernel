use crate::arch::syscall::nr;
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct SchedYieldSyscall => nr::SCHED_YIELD, "sched_yield", |_ctx, _args| {
        SyscallDisposition::Return(Ok(0))
    }
);
