use alloc::sync::Arc;

use aether_vfs::{FileNode, OpenFlags, SharedMemoryFile};

use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext, anonymous_filesystem_identity};
use crate::syscall::SyscallDisposition;
use crate::syscall::abi::read_path;

crate::declare_syscall!(
    pub struct MemfdCreateSyscall => nr::MEMFD_CREATE, "memfd_create", |ctx, args| {
        let Ok(name) = read_path(ctx, args.get(0), 249) else {
            return SyscallDisposition::Return(Err(SysErr::Fault));
        };
        SyscallDisposition::Return(ctx.memfd_create(&name, args.get(1)))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_memfd_create(&mut self, name: &str, flags: u64) -> SysResult<u64> {
        const MFD_CLOEXEC: u64 = 0x0001;
        const MFD_ALLOW_SEALING: u64 = 0x0002;
        const MFD_HUGETLB: u64 = 0x0004;

        if name.len() > 249 {
            return Err(SysErr::Inval);
        }
        if (flags & MFD_HUGETLB) != 0 {
            return Err(SysErr::NotSup);
        }
        if (flags & !(MFD_CLOEXEC | MFD_ALLOW_SEALING)) != 0 {
            return Err(SysErr::Inval);
        }

        let node = FileNode::new(
            if name.is_empty() { "memfd" } else { name },
            Arc::new(SharedMemoryFile::new_with_sealing(
                (flags & MFD_ALLOW_SEALING) != 0,
            )),
        );
        Ok(self.process.files.insert_node(
            node,
            OpenFlags::from_bits(OpenFlags::READ | OpenFlags::WRITE),
            anonymous_filesystem_identity(),
            None,
            (flags & MFD_CLOEXEC) != 0,
        ) as u64)
    }
}
