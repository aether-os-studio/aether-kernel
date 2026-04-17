use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(pub struct Signalfd4Syscall => nr::SIGNALFD4, "signalfd4", |ctx, args| {
    SyscallDisposition::Return(ctx.signalfd4(
        args.get(0) as i32,
        args.get(1),
        args.get(2) as usize,
        args.get(3),
    ))
});

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_signalfd4(
        &mut self,
        fd: i32,
        mask: u64,
        sigsetsize: usize,
        flags: u64,
    ) -> SysResult<u64> {
        const SFD_CLOEXEC: u64 = 0o2000000;
        const SFD_NONBLOCK: u64 = 0o4000;

        if sigsetsize != core::mem::size_of::<u64>() {
            return Err(SysErr::Inval);
        }
        if (flags & !(SFD_CLOEXEC | SFD_NONBLOCK)) != 0 {
            return Err(SysErr::Inval);
        }

        if mask == 0 {
            return Err(SysErr::Fault);
        }
        let raw_mask = self.syscall_read_user_exact_buffer(mask, core::mem::size_of::<u64>())?;
        let mask = u64::from_ne_bytes(raw_mask.try_into().map_err(|_| SysErr::Fault)?)
            & !(crate::signal::sigbit(crate::signal::SIGKILL)
                | crate::signal::sigbit(crate::signal::SIGSTOP));
        let mut open_flags = aether_vfs::OpenFlags::from_bits(aether_vfs::OpenFlags::READ);
        if (flags & SFD_NONBLOCK) != 0 {
            open_flags = aether_vfs::OpenFlags::from_bits(
                open_flags.bits() | aether_vfs::OpenFlags::NONBLOCK,
            );
        }

        if fd >= 0 {
            let descriptor = self.process.files.get_mut(fd as u32).ok_or(SysErr::BadFd)?;
            let node = descriptor.file.lock().node();
            let signalfd = node
                .file()
                .and_then(|file| file.as_any().downcast_ref::<crate::signal::SignalFdFile>())
                .ok_or(SysErr::Inval)?;
            signalfd.set_mask(mask);
            descriptor.file.lock().set_flags(open_flags);
            descriptor.cloexec = (flags & SFD_CLOEXEC) != 0;
            return Ok(fd as u64);
        }

        let node: aether_vfs::NodeRef = aether_vfs::FileNode::new(
            "signalfd",
            crate::signal::create_signalfd(self.process.signals.clone(), mask),
        );
        let filesystem = crate::process::anonymous_filesystem_identity();
        Ok(self.process.files.insert_node(
            node,
            open_flags,
            filesystem,
            None,
            (flags & SFD_CLOEXEC) != 0,
        ) as u64)
    }
}
