use crate::arch::syscall::nr;
use crate::errno::SysResult;
use crate::process::ProcessSyscallContext;
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct ShmgetSyscall => nr::SHMGET, "shmget", |ctx, args| {
        SyscallDisposition::Return(ctx.shmget(args.get(0) as i32, args.get(1) as usize, args.get(2) as i32))
    }
);

crate::declare_syscall!(
    pub struct ShmatSyscall => nr::SHMAT, "shmat", |ctx, args| {
        SyscallDisposition::Return(ctx.shmat(args.get(0) as i32, args.get(1), args.get(2) as i32))
    }
);

crate::declare_syscall!(
    pub struct ShmctlSyscall => nr::SHMCTL, "shmctl", |ctx, args| {
        SyscallDisposition::Return(ctx.shmctl(args.get(0) as i32, args.get(1) as i32, args.get(2)))
    }
);

crate::declare_syscall!(
    pub struct ShmdtSyscall => nr::SHMDT, "shmdt", |ctx, args| {
        SyscallDisposition::Return(ctx.shmdt(args.get(0)))
    }
);

impl ProcessSyscallContext<'_> {
    pub(crate) fn shmget(&mut self, key: i32, size: usize, shmflg: i32) -> SysResult<u64> {
        crate::fs::sysv_shmget(
            key,
            size,
            shmflg,
            &self.process.credentials,
            self.process.identity.pid as i32,
        )
    }

    pub(crate) fn shmat(&mut self, shmid: i32, shmaddr: u64, shmflg: i32) -> SysResult<u64> {
        crate::fs::sysv_shmat(self, shmid, shmaddr, shmflg)
    }

    pub(crate) fn shmctl(&mut self, shmid: i32, cmd: i32, buf: u64) -> SysResult<u64> {
        crate::fs::sysv_shmctl(self, shmid, cmd, buf)
    }

    pub(crate) fn shmdt(&mut self, shmaddr: u64) -> SysResult<u64> {
        crate::fs::sysv_shmdt(self.process, shmaddr)
    }
}
