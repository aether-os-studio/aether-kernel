use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct TgkillSyscall => nr::TGKILL, "tgkill", |ctx, args| {
        SyscallDisposition::Return(ctx.tgkill(args.get(0) as i32, args.get(1) as i32, args.get(2)))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_tgkill(&mut self, tgid: i32, pid: i32, signal: u64) -> SysResult<u64> {
        if tgid <= 0 || pid <= 0 {
            return Err(SysErr::Inval);
        }

        let signal = signal as u8;
        if signal as usize > crate::signal::SIGNAL_MAX {
            return Err(SysErr::Inval);
        }

        if self.services.thread_group_of(pid as u32) != Some(tgid as u32) {
            return Err(SysErr::NoEnt);
        }
        if signal == 0 {
            return Ok(0);
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
