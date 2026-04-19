use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct TkillSyscall => nr::TKILL, "tkill", |ctx, args| {
        SyscallDisposition::Return(ctx.tkill(args.get(0) as i32, args.get(1)))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_tkill(&mut self, pid: i32, signal: u64) -> SysResult<u64> {
        if pid <= 0 {
            return Err(SysErr::Inval);
        }

        let signal = signal as u8;
        if signal as usize > crate::signal::SIGNAL_MAX {
            return Err(SysErr::Inval);
        }
        if signal == 0 {
            return self
                .services
                .thread_group_of(pid as u32)
                .is_some()
                .then_some(0)
                .ok_or(SysErr::NoEnt);
        }

        self.services
            .send_kernel_signal(pid as u32, crate::signal::SignalInfo::kernel(signal, 0))
            .then_some(0)
            .ok_or(SysErr::NoEnt)
    }
}
