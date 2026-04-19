use aether_vfs::SharedMemoryFile;

use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::{KernelSyscallContext, SyscallDisposition};

crate::declare_syscall!(
    pub struct FcntlSyscall => nr::FCNTL, "fcntl", |ctx, args| { SyscallDisposition::Return(ctx.fcntl(args.get(0), args.get(1), args.get(2))) }
);

#[derive(Debug, Clone, Copy)]
struct LinuxFlock {
    l_type: i16,
    l_whence: i16,
    l_start: i64,
    l_len: i64,
    l_pid: i32,
}

impl LinuxFlock {
    const SIZE: usize = 24;

    fn read_from<S: ProcessServices>(
        ctx: &ProcessSyscallContext<'_, S>,
        address: u64,
    ) -> SysResult<Self> {
        let bytes = ctx.syscall_read_user_exact_buffer(address, Self::SIZE)?;
        Ok(Self {
            l_type: i16::from_ne_bytes(bytes[0..2].try_into().map_err(|_| SysErr::Fault)?),
            l_whence: i16::from_ne_bytes(bytes[2..4].try_into().map_err(|_| SysErr::Fault)?),
            l_start: i64::from_ne_bytes(bytes[4..12].try_into().map_err(|_| SysErr::Fault)?),
            l_len: i64::from_ne_bytes(bytes[12..20].try_into().map_err(|_| SysErr::Fault)?),
            l_pid: i32::from_ne_bytes(bytes[20..24].try_into().map_err(|_| SysErr::Fault)?),
        })
    }

    fn write_to<S: ProcessServices>(
        self,
        ctx: &mut ProcessSyscallContext<'_, S>,
        address: u64,
    ) -> SysResult<()> {
        let mut bytes = [0u8; Self::SIZE];
        bytes[0..2].copy_from_slice(&self.l_type.to_ne_bytes());
        bytes[2..4].copy_from_slice(&self.l_whence.to_ne_bytes());
        bytes[4..12].copy_from_slice(&self.l_start.to_ne_bytes());
        bytes[12..20].copy_from_slice(&self.l_len.to_ne_bytes());
        bytes[20..24].copy_from_slice(&self.l_pid.to_ne_bytes());
        ctx.write_user_buffer(address, &bytes)
    }
}

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_fcntl(&mut self, fd: u64, command: u64, arg: u64) -> SysResult<u64> {
        const F_DUPFD: u64 = 0;
        const F_GETFD: u64 = 1;
        const F_SETFD: u64 = 2;
        const F_GETFL: u64 = 3;
        const F_SETFL: u64 = 4;
        const F_GETLK: u64 = 5;
        const F_SETLK: u64 = 6;
        const F_SETLKW: u64 = 7;
        const F_ADD_SEALS: u64 = 1033;
        const F_GET_SEALS: u64 = 1034;
        const F_DUPFD_CLOEXEC: u64 = 1030;
        const F_RDLCK: i16 = 0;
        const F_WRLCK: i16 = 1;
        const F_UNLCK: i16 = 2;
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
            F_GETLK => {
                let _descriptor = self.process.files.get(fd as u32).ok_or(SysErr::BadFd)?;
                let mut flock = LinuxFlock::read_from(self, arg)?;
                if !matches!(flock.l_type, F_RDLCK | F_WRLCK | F_UNLCK) {
                    return Err(SysErr::Inval);
                }
                // Report "no conflicting lock" for now so userspace does not fail on unsupported
                // record locking while the kernel still uses whole-file flock semantics.
                flock.l_type = F_UNLCK;
                flock.l_pid = 0;
                flock.write_to(self, arg)?;
                Ok(0)
            }
            F_SETLK | F_SETLKW => {
                let _descriptor = self.process.files.get(fd as u32).ok_or(SysErr::BadFd)?;
                let flock = LinuxFlock::read_from(self, arg)?;
                if !matches!(flock.l_type, F_RDLCK | F_WRLCK | F_UNLCK) {
                    return Err(SysErr::Inval);
                }
                Ok(0)
            }
            F_ADD_SEALS => {
                let descriptor = self.process.files.get(fd as u32).ok_or(SysErr::BadFd)?;
                let file = descriptor.file.lock();
                let node = file.node();
                let Some(shared) = node
                    .file()
                    .and_then(|ops| ops.as_any().downcast_ref::<SharedMemoryFile>())
                else {
                    return Err(SysErr::Inval);
                };
                shared.add_seals(arg as u32).map_err(SysErr::from)?;
                Ok(0)
            }
            F_GET_SEALS => {
                let descriptor = self.process.files.get(fd as u32).ok_or(SysErr::BadFd)?;
                let file = descriptor.file.lock();
                let node = file.node();
                let Some(shared) = node
                    .file()
                    .and_then(|ops| ops.as_any().downcast_ref::<SharedMemoryFile>())
                else {
                    return Err(SysErr::Inval);
                };
                Ok(shared.seals() as u64)
            }
            _ => Err(SysErr::Inval),
        }
    }
}
