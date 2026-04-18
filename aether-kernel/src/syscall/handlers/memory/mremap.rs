use aether_frame::mm::{MapFlags, PAGE_SIZE};

use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{MmapRegion, MmapRegionBacking, ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

const MREMAP_MAYMOVE: u64 = 0x1;
const MREMAP_FIXED: u64 = 0x2;
const MREMAP_DONTUNMAP: u64 = 0x4;
const MAP_SHARED: u64 = 0x01;
const MAP_FIXED: u64 = 0x10;
const PROT_READ: u64 = 0x1;
const COPY_CHUNK: usize = 64 * 1024;

crate::declare_syscall!(
    pub struct MremapSyscall => nr::MREMAP, "mremap", |ctx, args| {
        SyscallDisposition::Return(ctx.mremap(
            args.get(0),
            args.get(1),
            args.get(2),
            args.get(3),
            args.get(4),
        ))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    fn mmap_prot_from_page_flags(flags: MapFlags) -> u64 {
        let mut prot = PROT_READ;
        if flags.contains(MapFlags::WRITE) {
            prot |= 0x2;
        }
        if flags.contains(MapFlags::EXECUTE) {
            prot |= 0x4;
        }
        prot
    }

    fn map_region_backing(
        &mut self,
        region: &MmapRegion,
        address: u64,
        len: u64,
        offset_delta: u64,
        fixed: bool,
    ) -> SysResult<u64> {
        let flags = if fixed {
            region.mmap_flags | MAP_FIXED
        } else {
            region.mmap_flags
        };
        let prot = Self::mmap_prot_from_page_flags(region.page_flags);

        match &region.backing {
            MmapRegionBacking::Anonymous => {
                Self::map_anonymous_region(self.process, address, len, prot, flags, false)
            }
            MmapRegionBacking::BufferedFile { file, offset }
            | MmapRegionBacking::SharedFile { file, offset }
            | MmapRegionBacking::DirectFile { file, offset } => Self::map_file_region(
                self.process,
                file.clone(),
                address,
                len,
                prot,
                flags,
                offset.saturating_add(offset_delta),
                false,
            ),
        }
    }

    fn copy_user_range(&mut self, src: u64, dst: u64, len: u64) -> SysResult<()> {
        let mut copied = 0u64;
        while copied < len {
            let chunk = (len - copied).min(COPY_CHUNK as u64) as usize;
            let buffer = self
                .process
                .task
                .address_space
                .read_user_exact(src.saturating_add(copied), chunk)
                .map_err(SysErr::from)?;
            let written = self
                .process
                .task
                .address_space
                .write(dst.saturating_add(copied), &buffer)
                .map_err(SysErr::from)?;
            if written != buffer.len() {
                return Err(SysErr::Fault);
            }
            copied = copied.saturating_add(chunk as u64);
        }
        Ok(())
    }

    fn should_copy_data(region: &MmapRegion) -> bool {
        matches!(
            region.backing,
            MmapRegionBacking::Anonymous | MmapRegionBacking::BufferedFile { .. }
        )
    }

    fn cleanup_mremap_target(&mut self, address: u64, len: u64) {
        let _ = self.process.task.address_space.munmap(address, len);
        self.process
            .remove_mmap_region_range(address, address.saturating_add(len));
    }

    pub(crate) fn syscall_mremap(
        &mut self,
        old_address: u64,
        old_size: u64,
        new_size: u64,
        flags: u64,
        new_address: u64,
    ) -> SysResult<u64> {
        if (flags & !(MREMAP_MAYMOVE | MREMAP_FIXED | MREMAP_DONTUNMAP)) != 0 {
            return Err(SysErr::Inval);
        }
        if (flags & MREMAP_DONTUNMAP) != 0 && (flags & MREMAP_MAYMOVE) == 0 {
            return Err(SysErr::Inval);
        }
        if old_size == 0 || new_size == 0 || !old_address.is_multiple_of(PAGE_SIZE) {
            // Linux has a special old_size==0 clone form for shareable mappings. That ABI path is
            // not implemented here yet because it needs separate duplicate-mapping bookkeeping.
            return Err(SysErr::Inval);
        }
        if (flags & MREMAP_FIXED) != 0 {
            if (flags & MREMAP_MAYMOVE) == 0
                || new_address == 0
                || !new_address.is_multiple_of(PAGE_SIZE)
            {
                return Err(SysErr::Inval);
            }
        } else if new_address != 0 {
            return Err(SysErr::Inval);
        }

        let old_len = old_size.div_ceil(PAGE_SIZE) * PAGE_SIZE;
        let new_len = new_size.div_ceil(PAGE_SIZE) * PAGE_SIZE;
        if (flags & MREMAP_DONTUNMAP) != 0 && old_len != new_len {
            return Err(SysErr::Inval);
        }
        let old_end = old_address.saturating_add(old_len);

        let region = self
            .process
            .slice_mmap_region(old_address, old_end)
            .ok_or(SysErr::Fault)?;
        if (flags & MREMAP_DONTUNMAP) != 0
            && (!matches!(region.backing, MmapRegionBacking::Anonymous)
                || (region.mmap_flags & MAP_SHARED) != 0)
        {
            // This kernel only supports the Linux-style restricted DONTUNMAP form for
            // non-shared anonymous mappings. File/shared mappings need userfaultfd-like follow-up.
            return Err(SysErr::Inval);
        }

        if new_len == old_len
            && (flags & MREMAP_DONTUNMAP) == 0
            && ((flags & MREMAP_FIXED) == 0 || new_address == old_address)
        {
            return Ok(old_address);
        }

        let fixed_target = ((flags & MREMAP_FIXED) != 0).then_some(new_address);
        if let Some(target) = fixed_target {
            let target_end = target.saturating_add(new_len);
            if target < old_end && old_address < target_end {
                return Err(SysErr::Inval);
            }
        }

        if fixed_target.is_none() && new_len < old_len {
            self.process
                .task
                .address_space
                .munmap(old_address.saturating_add(new_len), old_len - new_len)
                .map_err(SysErr::from)?;
            self.process.remove_mmap_region_range(old_address, old_end);
            self.process.insert_mmap_region(MmapRegion {
                start: old_address,
                end: old_address.saturating_add(new_len),
                ..region
            });
            return Ok(old_address);
        }

        if fixed_target.is_none() && new_len > old_len {
            let extra_len = new_len - old_len;
            if self
                .process
                .task
                .address_space
                .is_range_free(old_end, extra_len)
            {
                self.map_region_backing(&region, old_end, extra_len, old_len, true)?;
                self.process.remove_mmap_region_range(old_address, old_end);
                self.process.insert_mmap_region(MmapRegion {
                    start: old_address,
                    end: old_address.saturating_add(new_len),
                    ..region
                });
                return Ok(old_address);
            }
        }

        if (flags & MREMAP_MAYMOVE) == 0 {
            return Err(SysErr::NoMem);
        }

        let target = self.map_region_backing(
            &region,
            fixed_target.unwrap_or(0),
            new_len,
            0,
            fixed_target.is_some(),
        )?;

        if Self::should_copy_data(&region) && region.page_flags.contains(MapFlags::READ) {
            let copy_len = old_size.min(new_size);
            let restore_flags = region.page_flags;
            let needs_temp_write = copy_len != 0 && !restore_flags.contains(MapFlags::WRITE);
            if needs_temp_write
                && let Err(error) = self
                    .process
                    .task
                    .address_space
                    .mprotect(target, copy_len, restore_flags | MapFlags::WRITE)
                    .map_err(SysErr::from)
            {
                self.cleanup_mremap_target(target, new_len);
                return Err(error);
            }
            if let Err(error) = self.copy_user_range(old_address, target, copy_len) {
                self.cleanup_mremap_target(target, new_len);
                return Err(error);
            }
            if needs_temp_write
                && let Err(error) = self
                    .process
                    .task
                    .address_space
                    .mprotect(target, copy_len, restore_flags)
                    .map_err(SysErr::from)
            {
                self.cleanup_mremap_target(target, new_len);
                return Err(error);
            }
        }

        if (flags & MREMAP_DONTUNMAP) == 0
            && let Err(error) = self
                .process
                .task
                .address_space
                .munmap(old_address, old_len)
                .map_err(SysErr::from)
        {
            self.cleanup_mremap_target(target, new_len);
            return Err(error);
        }
        self.process
            .remove_mmap_region_range(target, target.saturating_add(new_len));
        self.process.remove_mmap_region_range(old_address, old_end);
        self.process.insert_mmap_region(MmapRegion {
            start: target,
            end: target.saturating_add(new_len),
            ..region
        });
        Ok(target)
    }
}
