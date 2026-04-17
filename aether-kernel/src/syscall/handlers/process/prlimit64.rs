use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::{KernelSyscallContext, SyscallDisposition};

crate::declare_syscall!(
    pub struct Prlimit64Syscall => nr::PRLIMIT64, "prlimit64", |ctx, args| {
        let pid = args.get(0) as i32;
        let resource = args.get(1);
        let new_limit = args.get(2);
        let old_limit = args.get(3);
        SyscallDisposition::Return(ctx.prlimit64(pid, resource, new_limit, old_limit))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_prlimit64(
        &mut self,
        pid: i32,
        resource: u64,
        new_limit: u64,
        old_limit: u64,
    ) -> SysResult<u64> {
        if pid != 0 && pid as u32 != self.process.identity.pid {
            return Err(SysErr::Srch);
        }

        const RLIMIT_CPU: u64 = 0;
        const RLIMIT_FSIZE: u64 = 1;
        const RLIMIT_DATA: u64 = 2;
        const RLIMIT_STACK: u64 = 3;
        const RLIMIT_CORE: u64 = 4;
        const RLIMIT_RSS: u64 = 5;
        const RLIMIT_NPROC: u64 = 6;
        const RLIMIT_NOFILE: u64 = 7;
        const RLIMIT_MEMLOCK: u64 = 8;
        const RLIMIT_AS: u64 = 9;
        const RLIMIT_LOCKS: u64 = 10;
        const RLIMIT_SIGPENDING: u64 = 11;
        const RLIMIT_MSGQUEUE: u64 = 12;
        const RLIMIT_NICE: u64 = 13;
        const RLIMIT_RTPRIO: u64 = 14;
        const RLIMIT_RTTIME: u64 = 15;

        if resource > RLIMIT_RTTIME {
            return Err(SysErr::Inval);
        }

        if old_limit != 0 {
            let (cur, max) = match resource {
                RLIMIT_NOFILE => (1024u64, 4096u64),
                RLIMIT_STACK => (8 * 1024 * 1024u64, u64::MAX),
                RLIMIT_DATA => (u64::MAX, u64::MAX),
                RLIMIT_AS => (u64::MAX, u64::MAX),
                RLIMIT_CPU => (u64::MAX, u64::MAX),
                RLIMIT_FSIZE => (u64::MAX, u64::MAX),
                RLIMIT_CORE => (0u64, u64::MAX),
                RLIMIT_RSS => (u64::MAX, u64::MAX),
                RLIMIT_NPROC => (1024u64, 1024u64),
                RLIMIT_MEMLOCK => (64 * 1024 * 1024u64, 64 * 1024 * 1024u64),
                RLIMIT_LOCKS => (u64::MAX, u64::MAX),
                RLIMIT_SIGPENDING => (1024u64, 1024u64),
                RLIMIT_MSGQUEUE => (819200u64, 819200u64),
                RLIMIT_NICE => (0u64, 0u64),
                RLIMIT_RTPRIO => (0u64, 0u64),
                RLIMIT_RTTIME => (u64::MAX, u64::MAX),
                _ => (u64::MAX, u64::MAX),
            };

            let mut limit_bytes = [0u8; 16];
            limit_bytes[..8].copy_from_slice(&cur.to_ne_bytes());
            limit_bytes[8..].copy_from_slice(&max.to_ne_bytes());
            self.write_user_buffer(old_limit, &limit_bytes)?;
        }

        if new_limit != 0 {
            let new_bytes = self.syscall_read_user_exact_buffer(new_limit, 16)?;
            let new_cur = u64::from_ne_bytes(new_bytes[..8].try_into().unwrap_or([0; 8]));
            let new_max = u64::from_ne_bytes(new_bytes[8..].try_into().unwrap_or([0; 8]));

            if new_cur > new_max {
                return Err(SysErr::Inval);
            }
        }

        Ok(0)
    }
}
