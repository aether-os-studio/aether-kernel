use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext, anonymous_filesystem_identity};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct TimerfdCreateSyscall => nr::TIMERFD_CREATE, "timerfd_create", |ctx, args| {
        SyscallDisposition::Return(ctx.timerfd_create(args.get(0) as i32, args.get(1)))
    }
);

crate::declare_syscall!(
    pub struct TimerfdSettimeSyscall => nr::TIMERFD_SETTIME, "timerfd_settime", |ctx, args| {
        SyscallDisposition::Return(ctx.timerfd_settime(
            args.get(0) as i32,
            args.get(1),
            args.get(2),
            args.get(3),
        ))
    }
);

crate::declare_syscall!(
    pub struct TimerfdGettimeSyscall => nr::TIMERFD_GETTIME, "timerfd_gettime", |ctx, args| {
        SyscallDisposition::Return(ctx.timerfd_gettime(args.get(0) as i32, args.get(1)))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_timerfd_create(&mut self, clockid: i32, flags: u64) -> SysResult<u64> {
        if (flags & !crate::fs::TFD_CREATE_FLAGS) != 0 {
            return Err(SysErr::Inval);
        }

        let clock = crate::fs::parse_timerfd_clock(clockid)?;
        let mut open_flags = aether_vfs::OpenFlags::from_bits(aether_vfs::OpenFlags::READ);
        if (flags & crate::fs::TFD_NONBLOCK) != 0 {
            open_flags = aether_vfs::OpenFlags::from_bits(
                open_flags.bits() | aether_vfs::OpenFlags::NONBLOCK,
            );
        }

        let node: aether_vfs::NodeRef =
            aether_vfs::FileNode::new("timerfd", crate::fs::TimerFdFile::create(clock));
        Ok(self.process.files.insert_node(
            node,
            open_flags,
            anonymous_filesystem_identity(),
            None,
            (flags & crate::fs::TFD_CLOEXEC) != 0,
        ) as u64)
    }

    pub(crate) fn syscall_timerfd_settime(
        &mut self,
        fd: i32,
        flags: u64,
        new_value: u64,
        old_value: u64,
    ) -> SysResult<u64> {
        if fd < 0 {
            return Err(SysErr::BadFd);
        }
        if new_value == 0 {
            return Err(SysErr::Fault);
        }
        if (flags & !crate::fs::TFD_SETTIME_FLAGS) != 0 {
            return Err(SysErr::Inval);
        }

        let descriptor = self.process.files.get(fd as u32).ok_or(SysErr::BadFd)?;
        let node = descriptor.file.lock().node();
        let timerfd = node
            .file()
            .and_then(|file| file.as_any().downcast_ref::<crate::fs::TimerFdFile>())
            .ok_or(SysErr::Inval)?;

        let new_value = crate::fs::LinuxItimerSpec::read_from(self, new_value)?;
        let old_value_spec = timerfd.set_time(flags, new_value)?;
        if old_value != 0 {
            old_value_spec.write_to(self, old_value)?;
        }
        Ok(0)
    }

    pub(crate) fn syscall_timerfd_gettime(&mut self, fd: i32, curr_value: u64) -> SysResult<u64> {
        if fd < 0 {
            return Err(SysErr::BadFd);
        }
        if curr_value == 0 {
            return Err(SysErr::Fault);
        }

        let descriptor = self.process.files.get(fd as u32).ok_or(SysErr::BadFd)?;
        let node = descriptor.file.lock().node();
        let timerfd = node
            .file()
            .and_then(|file| file.as_any().downcast_ref::<crate::fs::TimerFdFile>())
            .ok_or(SysErr::Inval)?;
        timerfd.get_time().write_to(self, curr_value)?;
        Ok(0)
    }
}
