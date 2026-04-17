use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct InotifyInit1Syscall => nr::INOTIFY_INIT1, "inotify_init1", |ctx, args| {
        SyscallDisposition::Return(ctx.inotify_init1(args.get(0)))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_inotify_init1(&mut self, flags: u64) -> SysResult<u64> {
        const IN_CLOEXEC: u64 = 0o2000000;
        const IN_NONBLOCK: u64 = 0o4000;

        if (flags & !crate::fs::INOTIFY_INIT1_VALID_FLAGS) != 0 {
            return Err(SysErr::Inval);
        }

        let node: aether_vfs::NodeRef =
            aether_vfs::FileNode::new("inotify", crate::fs::create_inotify_instance());
        let filesystem = crate::process::anonymous_filesystem_identity();
        let mut open_flags = aether_vfs::OpenFlags::from_bits(aether_vfs::OpenFlags::READ);
        if (flags & IN_NONBLOCK) != 0 {
            open_flags = aether_vfs::OpenFlags::from_bits(
                open_flags.bits() | aether_vfs::OpenFlags::NONBLOCK,
            );
        }
        Ok(self.process.files.insert_node(
            node,
            open_flags,
            filesystem,
            None,
            (flags & IN_CLOEXEC) != 0,
        ) as u64)
    }
}
