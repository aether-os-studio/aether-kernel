use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ChildEvent, ChildEventKind, ProcessServices, ProcessSyscallContext};
use crate::signal::{self, CLD_CONTINUED, CLD_EXITED, CLD_KILLED, CLD_STOPPED};
use crate::syscall::{BlockResult, KernelSyscallContext, SyscallDisposition};

const P_ALL: i32 = 0;
const P_PID: i32 = 1;
const P_PGID: i32 = 2;
const P_PIDFD: i32 = 3;

const WNOHANG: u64 = 1;
const WSTOPPED: u64 = 2;
const WEXITED: u64 = 4;
const WCONTINUED: u64 = 8;
const WNOWAIT: u64 = 0x0100_0000;
const ALLOWED_WAITID_FLAGS: u64 = WNOHANG | WSTOPPED | WEXITED | WCONTINUED | WNOWAIT;

crate::declare_syscall!(
    pub struct WaitidSyscall => nr::WAITID, "waitid", |ctx, args| {
        ctx.waitid_blocking(
            args.get(0) as i32,
            args.get(1),
            args.get(2),
            args.get(3),
            args.get(4),
        )
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    fn waitid_selector(
        &mut self,
        idtype: i32,
        id: u64,
    ) -> SysResult<crate::process::WaitChildSelector> {
        match idtype {
            P_ALL => Ok(crate::process::WaitChildSelector::Any),
            P_PID => {
                let pid = u32::try_from(id).map_err(|_| SysErr::Inval)?;
                Ok(crate::process::WaitChildSelector::Pid(pid))
            }
            P_PGID => {
                let process_group = if id == 0 {
                    self.process.identity.process_group
                } else {
                    u32::try_from(id).map_err(|_| SysErr::Inval)?
                };
                Ok(crate::process::WaitChildSelector::ProcessGroup(
                    process_group,
                ))
            }
            P_PIDFD => {
                let fd = i32::try_from(id).map_err(|_| SysErr::Inval)?;
                if fd < 0 {
                    return Err(SysErr::Inval);
                }
                let pid = self
                    .process
                    .files
                    .with_descriptor_mut(fd as u32, |descriptor| {
                        let file = descriptor.file.lock();
                        let pidfd = file
                            .file_ops()
                            .and_then(|ops| ops.as_any().downcast_ref::<crate::fs::PidFdFile>())
                            .ok_or(SysErr::BadFd)?;
                        Ok::<u32, SysErr>(pidfd.handle().pid())
                    })
                    .ok_or(SysErr::BadFd)??;
                Ok(crate::process::WaitChildSelector::Pid(pid))
            }
            _ => Err(SysErr::Inval),
        }
    }

    fn write_waitid_siginfo(&mut self, address: u64, event: ChildEvent) -> SysResult<()> {
        if address == 0 {
            return Ok(());
        }

        let mut bytes = [0u8; 128];
        let (code, status) = match event.kind {
            ChildEventKind::Exited(status) => {
                if status >= 128 {
                    // TODO: report `CLD_DUMPED` and the core-dump bit once the kernel tracks it.
                    (CLD_KILLED, status - 128)
                } else {
                    (CLD_EXITED, status)
                }
            }
            ChildEventKind::Stopped(signal_number) => (CLD_STOPPED, signal_number as i32),
            ChildEventKind::Continued => {
                // TODO: confirm whether Linux user space here expects `0` or `SIGCONT`.
                (CLD_CONTINUED, 0)
            }
        };

        bytes[0..4].copy_from_slice(&(signal::SIGCHLD as i32).to_ne_bytes());
        bytes[4..8].copy_from_slice(&0i32.to_ne_bytes());
        bytes[8..12].copy_from_slice(&code.to_ne_bytes());
        bytes[16..20].copy_from_slice(&(event.pid as i32).to_ne_bytes());
        bytes[20..24].copy_from_slice(&0u32.to_ne_bytes());
        bytes[24..28].copy_from_slice(&status.to_ne_bytes());
        self.write_user_buffer(address, &bytes)?;
        Ok(())
    }

    fn write_waitid_nohang_siginfo(&mut self, address: u64) -> SysResult<()> {
        if address == 0 {
            return Ok(());
        }
        self.write_user_buffer(address, &[0u8; 128])?;
        Ok(())
    }

    pub(crate) fn syscall_waitid(
        &mut self,
        idtype: i32,
        id: u64,
        infop: u64,
        options: u64,
        _rusage: u64,
    ) -> SysResult<u64> {
        if (options & !ALLOWED_WAITID_FLAGS) != 0 {
            return Err(SysErr::Inval);
        }
        if (options & (WEXITED | WSTOPPED | WCONTINUED)) == 0 {
            return Err(SysErr::Inval);
        }

        let selector = self.waitid_selector(idtype, id)?;
        let consume = (options & WNOWAIT) == 0;
        if let Some(event) =
            self.services
                .wait_child_event(self.process.identity.pid, selector, options, consume)
        {
            self.write_waitid_siginfo(infop, event)?;
            // TODO: populate `rusage` with real child resource usage once the kernel tracks it.
            return Ok(0);
        }

        if (options & WNOHANG) != 0 {
            if self
                .services
                .has_waitable_child(self.process.identity.pid, selector)
            {
                self.write_waitid_nohang_siginfo(infop)?;
                return Ok(0);
            }
            return Err(SysErr::Child);
        }

        if self
            .services
            .has_waitable_child(self.process.identity.pid, selector)
        {
            return Err(SysErr::Again);
        }
        Err(SysErr::Child)
    }

    pub(crate) fn syscall_waitid_blocking(
        &mut self,
        idtype: i32,
        id: u64,
        infop: u64,
        options: u64,
        rusage: u64,
    ) -> SyscallDisposition {
        loop {
            if let Some(result) = self.process.wake_result.take() {
                match result {
                    BlockResult::CompletedValue { .. } => {}
                    BlockResult::SignalInterrupted => {
                        return SyscallDisposition::err(SysErr::Intr);
                    }
                    _ => return SyscallDisposition::err(SysErr::Intr),
                }
            }

            match self.syscall_waitid(idtype, id, infop, options, rusage) {
                Ok(value) => return SyscallDisposition::ok(value),
                Err(SysErr::Again) => {
                    let selector = match self.waitid_selector(idtype, id) {
                        Ok(selector) => selector,
                        Err(error) => return SyscallDisposition::err(error),
                    };
                    match self.wait_wait_child(
                        selector,
                        crate::process::WaitChildApi::WaitId,
                        0,
                        infop,
                        options,
                    ) {
                        Ok(BlockResult::CompletedValue { .. }) => {}
                        Ok(BlockResult::SignalInterrupted) => {
                            return SyscallDisposition::err(SysErr::Intr);
                        }
                        Ok(_) => return SyscallDisposition::err(SysErr::Intr),
                        Err(disposition) => return disposition,
                    }
                }
                Err(error) => return SyscallDisposition::err(error),
            }
        }
    }
}
