use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::{KernelSyscallContext, SyscallDisposition};

crate::declare_syscall!(pub struct PrctlSyscall => nr::PRCTL, "prctl", |ctx, args| {
    SyscallDisposition::Return(
        ctx.prctl(
            args.get(0),
            args.get(1),
            args.get(2),
            args.get(3),
            args.get(4),
        ),
    )
});

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_prctl(
        &mut self,
        option: u64,
        arg2: u64,
        arg3: u64,
        _arg4: u64,
        _arg5: u64,
    ) -> SysResult<u64> {
        const PR_SET_PDEATHSIG: u64 = 1;
        const PR_GET_PDEATHSIG: u64 = 2;
        const PR_GET_DUMPABLE: u64 = 3;
        const PR_SET_DUMPABLE: u64 = 4;
        const PR_GET_KEEPCAPS: u64 = 7;
        const PR_SET_KEEPCAPS: u64 = 8;
        const PR_SET_NAME: u64 = 15;
        const PR_GET_NAME: u64 = 16;
        const PR_CAPBSET_READ: u64 = 23;
        const PR_CAPBSET_DROP: u64 = 24;
        const PR_GET_SECUREBITS: u64 = 27;
        const PR_SET_CHILD_SUBREAPER: u64 = 36;
        const PR_GET_CHILD_SUBREAPER: u64 = 37;
        const PR_SET_NO_NEW_PRIVS: u64 = 38;
        const PR_GET_NO_NEW_PRIVS: u64 = 39;
        const PR_GET_TID_ADDRESS: u64 = 40;
        const PR_SET_THP_DISABLE: u64 = 41;
        const PR_GET_THP_DISABLE: u64 = 42;
        const PR_CAP_AMBIENT: u64 = 47;
        const PR_CAP_AMBIENT_IS_SET: u64 = 1;
        const PR_CAP_AMBIENT_RAISE: u64 = 2;
        const PR_CAP_AMBIENT_LOWER: u64 = 3;
        const PR_CAP_AMBIENT_CLEAR_ALL: u64 = 4;
        const PR_SET_TIMERSLACK: u64 = 29;
        const PR_GET_TIMERSLACK: u64 = 30;
        const CAP_LAST_CAP: u64 = 40;

        match option {
            PR_SET_PDEATHSIG => {
                if arg2 > crate::signal::SIGNAL_MAX as u64 {
                    return Err(SysErr::Inval);
                }
                self.process.prctl.parent_death_signal = arg2 as u8;
                Ok(0)
            }
            PR_GET_PDEATHSIG => {
                let signal = u32::from(self.process.prctl.parent_death_signal).to_ne_bytes();
                self.write_user_buffer(arg2, &signal)?;
                Ok(0)
            }
            PR_GET_DUMPABLE => Ok(self.process.prctl.dumpable as u64),
            PR_SET_DUMPABLE => match arg2 {
                0 => {
                    self.process.prctl.dumpable = false;
                    Ok(0)
                }
                1 => {
                    self.process.prctl.dumpable = true;
                    Ok(0)
                }
                _ => Err(SysErr::Inval),
            },
            PR_GET_KEEPCAPS => Ok(self.process.prctl.keepcaps as u64),
            PR_SET_KEEPCAPS => match arg2 {
                0 => {
                    self.process.prctl.keepcaps = false;
                    Ok(0)
                }
                1 => {
                    self.process.prctl.keepcaps = true;
                    Ok(0)
                }
                _ => Err(SysErr::Inval),
            },
            PR_SET_NAME => {
                let name = self.syscall_read_user_exact_buffer(arg2, 16)?;
                self.process.prctl.set_name_bytes(&name);
                Ok(0)
            }
            PR_GET_NAME => {
                let name = *self.process.prctl.name_bytes();
                self.write_user_buffer(arg2, &name)?;
                Ok(0)
            }
            PR_CAPBSET_READ => {
                if arg2 > CAP_LAST_CAP {
                    return Err(SysErr::Inval);
                }
                Ok((self.process.prctl.capability_bounding_set >> arg2) & 1)
            }
            PR_CAPBSET_DROP => {
                if arg2 > CAP_LAST_CAP {
                    return Err(SysErr::Inval);
                }
                if self.process.credentials.euid != 0 {
                    return Err(SysErr::Perm);
                }
                self.process.prctl.capability_bounding_set &= !(1u64 << arg2);
                Ok(0)
            }
            PR_GET_SECUREBITS => Ok(0),
            PR_SET_CHILD_SUBREAPER => match arg2 {
                0 => {
                    self.process.prctl.child_subreaper = false;
                    Ok(0)
                }
                1 => {
                    self.process.prctl.child_subreaper = true;
                    Ok(0)
                }
                _ => Err(SysErr::Inval),
            },
            PR_GET_CHILD_SUBREAPER => {
                let value = u32::from(self.process.prctl.child_subreaper).to_ne_bytes();
                self.write_user_buffer(arg2, &value)?;
                Ok(0)
            }
            PR_SET_NO_NEW_PRIVS => {
                if arg2 != 1 || arg3 != 0 {
                    return Err(SysErr::Inval);
                }
                self.process.prctl.no_new_privs = true;
                Ok(0)
            }
            PR_GET_NO_NEW_PRIVS => Ok(self.process.prctl.no_new_privs as u64),
            PR_GET_TID_ADDRESS => {
                let address = self.process.clear_child_tid.unwrap_or(0).to_ne_bytes();
                self.write_user_buffer(arg2, &address)?;
                Ok(0)
            }
            PR_SET_THP_DISABLE => match arg2 {
                0 => {
                    self.process.prctl.thp_disable = false;
                    Ok(0)
                }
                1 => {
                    self.process.prctl.thp_disable = true;
                    Ok(0)
                }
                _ => Err(SysErr::Inval),
            },
            PR_GET_THP_DISABLE => Ok(self.process.prctl.thp_disable as u64),
            PR_SET_TIMERSLACK => {
                self.process.prctl.timer_slack_nanos = arg2;
                Ok(0)
            }
            PR_GET_TIMERSLACK => Ok(self.process.prctl.timer_slack_nanos),
            PR_CAP_AMBIENT => match arg2 {
                PR_CAP_AMBIENT_IS_SET => {
                    if arg3 > CAP_LAST_CAP {
                        return Err(SysErr::Inval);
                    }
                    Ok(0)
                }
                PR_CAP_AMBIENT_LOWER | PR_CAP_AMBIENT_CLEAR_ALL => Ok(0),
                PR_CAP_AMBIENT_RAISE => Err(SysErr::Perm),
                _ => Err(SysErr::Inval),
            },
            _ => Err(SysErr::Inval),
        }
    }
}
