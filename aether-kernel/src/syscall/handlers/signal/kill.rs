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
        let signal = signal as u8;
        if signal as usize > crate::signal::SIGNAL_MAX {
            return Err(SysErr::Inval);
        }

        match pid {
            pid if pid > 0 => {
                if signal == 0 {
                    return self
                        .services
                        .has_thread_group(pid as u32)
                        .then_some(0)
                        .ok_or(SysErr::NoEnt);
                }
                self.services
                    .send_process_signal(pid as u32, crate::signal::SignalInfo::kernel(signal, 0))
                    .then_some(0)
                    .ok_or(SysErr::NoEnt)
            }
            0 => {
                let process_group = self.process.identity.process_group;
                if signal == 0 {
                    return self
                        .services
                        .has_process_group(process_group)
                        .then_some(0)
                        .ok_or(SysErr::NoEnt);
                }
                (self.services.send_process_group_signal(
                    process_group,
                    crate::signal::SignalInfo::kernel(signal, 0),
                ) > 0)
                    .then_some(0)
                    .ok_or(SysErr::NoEnt)
            }
            -1 => {
                if signal == 0 {
                    return Ok(0);
                }
                (self
                    .services
                    .send_signal_all(crate::signal::SignalInfo::kernel(signal, 0), None)
                    > 0)
                .then_some(0)
                .ok_or(SysErr::NoEnt)
            }
            negative => {
                let process_group = negative.unsigned_abs();
                if signal == 0 {
                    return self
                        .services
                        .has_process_group(process_group)
                        .then_some(0)
                        .ok_or(SysErr::NoEnt);
                }
                (self.services.send_process_group_signal(
                    process_group,
                    crate::signal::SignalInfo::kernel(signal, 0),
                ) > 0)
                    .then_some(0)
                    .ok_or(SysErr::NoEnt)
            }
        }
    }
}
