use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct FcntlSyscall => nr::FCNTL, "fcntl", |ctx, args| { SyscallDisposition::Return(ctx.fcntl(args.get(0), args.get(1), args.get(2))) }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_fcntl(&mut self, fd: u64, command: u64, arg: u64) -> SysResult<u64> {
        const F_DUPFD: u64 = 0;
        const F_GETFD: u64 = 1;
        const F_SETFD: u64 = 2;
        const F_GETFL: u64 = 3;
        const F_SETFL: u64 = 4;
        const F_DUPFD_CLOEXEC: u64 = 1030;
        const FD_CLOEXEC: u64 = 1;
        const O_APPEND: u64 = 0o2000;
        const O_NONBLOCK: u64 = 0o4000;

        match command {
            F_DUPFD => self
                .process
                .files
                .duplicate(fd as u32, arg as u32, false)
                .map(u64::from)
                .ok_or(SysErr::BadFd),
            F_DUPFD_CLOEXEC => self
                .process
                .files
                .duplicate(fd as u32, arg as u32, true)
                .map(u64::from)
                .ok_or(SysErr::BadFd),
            F_GETFD => self
                .process
                .files
                .get(fd as u32)
                .map(|descriptor| if descriptor.cloexec { FD_CLOEXEC } else { 0 })
                .ok_or(SysErr::BadFd),
            F_SETFD => {
                let descriptor = self.process.files.get_mut(fd as u32).ok_or(SysErr::BadFd)?;
                descriptor.cloexec = (arg & FD_CLOEXEC) != 0;
                Ok(0)
            }
            F_GETFL => self
                .process
                .files
                .get(fd as u32)
                .map(|descriptor| crate::fs::linux_status_flags(descriptor.file.lock().flags()))
                .ok_or(SysErr::BadFd),
            F_SETFL => {
                let descriptor = self.process.files.get(fd as u32).ok_or(SysErr::BadFd)?;
                let mut file = descriptor.file.lock();
                let current = file.flags();
                let mut next = current.bits()
                    & !(aether_vfs::OpenFlags::APPEND | aether_vfs::OpenFlags::NONBLOCK);
                if (arg & O_APPEND) != 0 {
                    next |= aether_vfs::OpenFlags::APPEND;
                }
                if (arg & O_NONBLOCK) != 0 {
                    next |= aether_vfs::OpenFlags::NONBLOCK;
                }
                file.set_flags(aether_vfs::OpenFlags::from_bits(next));
                Ok(0)
            }
            _ => Err(SysErr::Inval),
        }
    }
}
