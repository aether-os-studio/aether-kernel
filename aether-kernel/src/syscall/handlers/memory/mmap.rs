use alloc::vec;

use aether_frame::mm::MapFlags;
use aether_frame::mm::PAGE_SIZE;
use aether_vfs::{MmapCachePolicy, MmapKind, MmapRequest};

use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{
    KernelProcess, MmapRegion, MmapRegionBacking, ProcessServices, ProcessSyscallContext,
};
use crate::syscall::SyscallDisposition;

const PROT_WRITE: u64 = 0x2;
const PROT_EXEC: u64 = 0x4;
const MAP_ANONYMOUS: u64 = 0x20;

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
    pub(crate) fn mmap_page_flags(prot: u64) -> MapFlags {
        let mut page_flags = MapFlags::READ | MapFlags::USER;
        if (prot & PROT_WRITE) != 0 {
            page_flags = page_flags | MapFlags::WRITE;
        }
        if (prot & PROT_EXEC) != 0 {
            page_flags = page_flags | MapFlags::EXECUTE;
        }
        page_flags
    }

    pub(crate) fn record_mmap_region(
        process: &mut KernelProcess,
        start: u64,
        len: u64,
        page_flags: MapFlags,
        mmap_flags: u64,
        backing: MmapRegionBacking,
    ) {
        let aligned_len = len.div_ceil(PAGE_SIZE) * PAGE_SIZE;
        process.insert_mmap_region(MmapRegion {
            start,
            end: start.saturating_add(aligned_len),
            page_flags,
            mmap_flags,
            backing,
        });
    }

    pub(crate) fn map_buffered_file(
        process: &mut KernelProcess,
        node: aether_vfs::NodeRef,
        address: u64,
        len: u64,
        flags: u64,
        page_flags: MapFlags,
        offset: u64,
    ) -> SysResult<u64> {
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

        process
            .task
            .address_space
            .mmap_bytes(address, len, flags, page_flags, &bytes)
            .map_err(SysErr::from)
    }

    pub(crate) fn map_file_region(
        process: &mut KernelProcess,
        file: aether_vfs::SharedOpenFile,
        address: u64,
        len: u64,
        prot: u64,
        flags: u64,
        offset: u64,
        record_region: bool,
    ) -> SysResult<u64> {
        if !offset.is_multiple_of(PAGE_SIZE) {
            return Err(SysErr::Inval);
        }

        let page_flags = Self::mmap_page_flags(prot);
        let (response, node) = {
            let file_guard = file.lock();
            let response = file_guard
                .mmap(MmapRequest {
                    offset,
                    length: len,
                    prot,
                    flags,
                })
                .map_err(SysErr::from)?;
            (response, file_guard.node())
        };

        let (mapped, backing) = match response.kind {
            MmapKind::Buffered => (
                Self::map_buffered_file(process, node, address, len, flags, page_flags, offset)?,
                MmapRegionBacking::BufferedFile {
                    file: file.clone(),
                    offset,
                },
            ),
            MmapKind::DirectPhysical {
                physical_address,
                cache_policy,
            } => {
                let device_flags = match cache_policy {
                    MmapCachePolicy::Cached => MapFlags::empty(),
                    MmapCachePolicy::Uncached => MapFlags::NO_CACHE,
                    MmapCachePolicy::WriteThrough => MapFlags::WRITE_THROUGH,
                };
                (
                    process
                        .task
                        .address_space
                        .mmap_physical(
                            address,
                            len,
                            flags,
                            page_flags | device_flags,
                            physical_address,
                        )
                        .map_err(SysErr::from)?,
                    MmapRegionBacking::DirectFile {
                        file: file.clone(),
                        offset,
                    },
                )
            }
            MmapKind::SharedPhysical {
                physical_pages,
                cache_policy,
            } => {
                let device_flags = match cache_policy {
                    MmapCachePolicy::Cached => MapFlags::empty(),
                    MmapCachePolicy::Uncached => MapFlags::NO_CACHE,
                    MmapCachePolicy::WriteThrough => MapFlags::WRITE_THROUGH,
                };
                (
                    process
                        .task
                        .address_space
                        .mmap_shared_physical(
                            address,
                            len,
                            flags,
                            page_flags | device_flags,
                            physical_pages.as_ref(),
                        )
                        .map_err(SysErr::from)?,
                    MmapRegionBacking::SharedFile {
                        file: file.clone(),
                        offset,
                    },
                )
            }
        };

        if record_region {
            Self::record_mmap_region(process, mapped, len, page_flags, flags, backing);
        }
        Ok(mapped)
    }

    pub(crate) fn map_anonymous_region(
        process: &mut KernelProcess,
        address: u64,
        len: u64,
        prot: u64,
        flags: u64,
        record_region: bool,
    ) -> SysResult<u64> {
        let page_flags = Self::mmap_page_flags(prot);
        let mapped = process
            .task
            .address_space
            .mmap_anonymous(address, len, flags, page_flags)
            .map_err(SysErr::from)?;
        if record_region {
            Self::record_mmap_region(
                process,
                mapped,
                len,
                page_flags,
                flags,
                MmapRegionBacking::Anonymous,
            );
        }
        Ok(mapped)
    }

    pub(crate) fn syscall_mmap(
        &mut self,
        address: u64,
        len: u64,
        prot: u64,
        flags: u64,
        fd: u64,
        offset: u64,
    ) -> SysResult<u64> {
        if (flags & MAP_ANONYMOUS) != 0 {
            return Self::map_anonymous_region(self.process, address, len, prot, flags, true);
        }

        let descriptor = self.process.files.get(fd as u32).ok_or(SysErr::BadFd)?;
        Self::map_file_region(
            self.process,
            descriptor.file.clone(),
            address,
            len,
            prot,
            flags,
            offset,
            true,
        )
    }
}
