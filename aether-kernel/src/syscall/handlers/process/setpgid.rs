use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{Pid, ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct SetPgidSyscall => nr::SETPGID, "setpgid", |ctx, args| {
        SyscallDisposition::Return(ctx.setpgid(
            crate::syscall::abi::arg_i32(args.get(0)),
            crate::syscall::abi::arg_i32(args.get(1)),
        ))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_setpgid(&mut self, pid: i32, pgid: i32) -> SysResult<u64> {
        if pid < 0 || pgid < 0 {
            return Err(SysErr::Inval);
        }

        let pid = pid as Pid;
        let pgid = pgid as Pid;
        self.services.setpgid(self.process, pid, pgid)
    }
}
