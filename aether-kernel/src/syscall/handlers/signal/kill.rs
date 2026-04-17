use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct KillSyscall => nr::KILL, "kill", |ctx, args| {
        SyscallDisposition::Return(ctx.send_signal(args.get(0) as i32, args.get(1)))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_send_signal(&mut self, pid: i32, signal: u64) -> SysResult<u64> {
        if pid <= 0 {
            return Err(SysErr::NoSys);
        }
        let signal = signal as u8;
        if signal == 0 || signal as usize > crate::signal::SIGNAL_MAX {
            return Err(SysErr::Inval);
        }

        if self
            .services
            .send_kernel_signal(pid as u32, crate::signal::SignalInfo::kernel(signal, 0))
        {
            Ok(0)
        } else {
            Err(SysErr::NoEnt)
        }
    }
}
