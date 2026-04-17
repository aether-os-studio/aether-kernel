use alloc::vec;

use aether_frame::mm::MapFlags;
use aether_vfs::{MmapCachePolicy, MmapKind, MmapRequest};

use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct MmapSyscall => nr::MMAP, "mmap", |ctx, args| {
        SyscallDisposition::Return(ctx.mmap(
            args.get(0),
            args.get(1),
            args.get(2),
            args.get(3),
            args.get(4),
            args.get(5),
        ))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_mmap(
        &mut self,
        address: u64,
        len: u64,
        prot: u64,
        flags: u64,
        fd: u64,
        offset: u64,
    ) -> SysResult<u64> {
        const PROT_WRITE: u64 = 0x2;
        const PROT_EXEC: u64 = 0x4;
        const MAP_ANONYMOUS: u64 = 0x20;

        let mut page_flags = MapFlags::READ | MapFlags::USER;
        if (prot & PROT_WRITE) != 0 {
            page_flags = page_flags | MapFlags::WRITE;
        }
        if (prot & PROT_EXEC) != 0 {
            page_flags = page_flags | MapFlags::EXECUTE;
        }

        if (flags & MAP_ANONYMOUS) != 0 {
            return self
                .process
                .task
                .address_space
                .mmap_anonymous(address, len, flags, page_flags)
                .map_err(SysErr::from);
        }

        if (offset % aether_frame::mm::PAGE_SIZE) != 0 {
            return Err(SysErr::Inval);
        }

        let descriptor = self.process.files.get(fd as u32).ok_or(SysErr::BadFd)?;
        let file = descriptor.file.lock();
        match file.mmap(MmapRequest {
            offset,
            length: len,
            prot,
            flags,
        }) {
            Ok(mapping) => match mapping.kind {
                MmapKind::Buffered => {}
                MmapKind::DirectPhysical {
                    physical_address,
                    cache_policy,
                } => {
                    let device_flags = match cache_policy {
                        MmapCachePolicy::Cached => MapFlags::empty(),
                        MmapCachePolicy::Uncached => MapFlags::NO_CACHE,
                        MmapCachePolicy::WriteThrough => MapFlags::WRITE_THROUGH,
                    };
                    return self
                        .process
                        .task
                        .address_space
                        .mmap_physical(
                            address,
                            len,
                            flags,
                            page_flags | device_flags,
                            physical_address,
                        )
                        .map_err(SysErr::from);
                }
            },
            Err(error) => return Err(SysErr::from(error)),
        }

        let node = file.node();
        let size = node.size();
        let start = core::cmp::min(offset as usize, size);
        let end = core::cmp::min(start.saturating_add(len as usize), size);
        let mut bytes = vec![0; end.saturating_sub(start)];
        let mut filled = 0usize;
        while filled < bytes.len() {
            let read = node
                .read(start + filled, &mut bytes[filled..])
                .map_err(SysErr::from)?;
            if read == 0 {
                break;
            }
            filled += read;
        }
        bytes.truncate(filled);

        self.process
            .task
            .address_space
            .mmap_bytes(address, len, flags, page_flags, &bytes)
            .map_err(SysErr::from)
    }
}
