use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(pub struct EventfdSyscall => nr::EVENTFD, "eventfd", |ctx, args| {
    SyscallDisposition::Return(ctx.eventfd(args.get(0) as u32))
});

crate::declare_syscall!(pub struct Eventfd2Syscall => nr::EVENTFD2, "eventfd2", |ctx, args| {
    SyscallDisposition::Return(ctx.eventfd2(args.get(0) as u32, args.get(1)))
});

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_eventfd(&mut self, initval: u32) -> SysResult<u64> {
        self.syscall_eventfd2(initval, 0)
    }

    pub(crate) fn syscall_eventfd2(&mut self, initval: u32, flags: u64) -> SysResult<u64> {
        const EFD_NONBLOCK: u64 = 0o0004000;
        const EFD_CLOEXEC: u64 = 0o2000000;

        if (flags & !crate::fs::EFD_VALID_FLAGS) != 0 {
            return Err(SysErr::Inval);
        }

        let mut open_flags = aether_vfs::OpenFlags::from_bits(
            aether_vfs::OpenFlags::READ | aether_vfs::OpenFlags::WRITE,
        );
        if (flags & EFD_NONBLOCK) != 0 {
            open_flags = aether_vfs::OpenFlags::from_bits(
                open_flags.bits() | aether_vfs::OpenFlags::NONBLOCK,
            );
        }

        let node: aether_vfs::NodeRef =
            aether_vfs::FileNode::new("eventfd", crate::fs::create_eventfd(initval, flags));
        let filesystem = crate::process::anonymous_filesystem_identity();
        Ok(self.process.files.insert_node(
            node,
            open_flags,
            filesystem,
            None,
            (flags & EFD_CLOEXEC) != 0,
        ) as u64)
    }
}
