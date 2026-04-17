use core::mem::size_of;
use core::ptr;
use core::slice;

use crate::boot::{MemoryMap, MemoryRegion, MemoryRegionKind, phys_to_virt};

use super::address::{PAGE_SHIFT, PAGE_SIZE, PhysAddr};
use super::frame::{FrameAllocError, FrameAllocator, PhysFrame};

const NONE: u32 = u32::MAX;
const MAX_ORDER: usize = u64::BITS as usize - PAGE_SHIFT;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuddyAllocatorError {
    NoUsableMemory,
    MetadataRegionMissing,
    TooManyFrames,
}

#[derive(Clone, Copy)]
#[repr(u8)]
enum FrameState {
    Unused = 0,
    Free = 1,
    Allocated = 2,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct FrameMeta {
    ref_count: u32,
    next: u32,
    order: u8,
    state: FrameState,
    _reserved: [u8; 2],
}

impl FrameMeta {
    const EMPTY: Self = Self {
        ref_count: 0,
        next: NONE,
        order: 0,
        state: FrameState::Unused,
        _reserved: [0; 2],
    };
}

#[derive(Clone, Copy)]
struct ReservedRange {
    start_frame: usize,
    frame_count: usize,
}

pub struct BuddyFrameAllocator {
    total_frames: usize,
    free_heads: [u32; MAX_ORDER + 1],
    metadata: &'static mut [FrameMeta],
}

impl BuddyFrameAllocator {
    /// Builds the initial buddy allocator state from the boot memory map.
    ///
    /// # Safety
    /// The caller must ensure the memory map is trustworthy and that no other
    /// allocator instance concurrently manages the same physical frames.
    pub unsafe fn bootstrap(memory_map: &MemoryMap<'_>) -> Result<Self, BuddyAllocatorError> {
        let highest_addr = memory_map
            .iter()
            .filter(|r| r.kind == MemoryRegionKind::USABLE)
            .map(region_end)
            .max()
            .ok_or(BuddyAllocatorError::NoUsableMemory)?;
        let total_frames = align_down(highest_addr, PAGE_SIZE) as usize >> PAGE_SHIFT;
        if total_frames > u32::MAX as usize {
            return Err(BuddyAllocatorError::TooManyFrames);
        }

        let metadata_bytes = total_frames * size_of::<FrameMeta>();
        let metadata_frames = div_ceil(metadata_bytes, PAGE_SIZE as usize);
        let metadata_phys = reserve_metadata(memory_map, metadata_frames)
            .ok_or(BuddyAllocatorError::MetadataRegionMissing)?;
        let metadata_ptr = phys_to_virt(metadata_phys.as_u64()) as *mut FrameMeta;

        unsafe {
            ptr::write_bytes(metadata_ptr, 0, total_frames);
        }

        let metadata = unsafe { slice::from_raw_parts_mut(metadata_ptr, total_frames) };
        metadata.fill(FrameMeta::EMPTY);

        let mut allocator = Self {
            total_frames,
            free_heads: [NONE; MAX_ORDER + 1],
            metadata,
        };

        let reserved = ReservedRange {
            start_frame: metadata_phys.as_u64() as usize >> PAGE_SHIFT,
            frame_count: metadata_frames,
        };

        for region in memory_map.iter().copied() {
            if region.kind != MemoryRegionKind::USABLE {
                continue;
            }

            allocator.add_usable_region(region, reserved);
        }

        Ok(allocator)
    }

    #[must_use]
    pub const fn max_order(&self) -> usize {
        MAX_ORDER
    }

    fn add_usable_region(&mut self, region: MemoryRegion, reserved: ReservedRange) {
        let start = align_up(region.start, PAGE_SIZE);
        let end = align_down(region.start.saturating_add(region.len), PAGE_SIZE);
        if end <= start {
            return;
        }

        let region_start_frame = (start >> PAGE_SHIFT) as usize;
        let region_end_frame = (end >> PAGE_SHIFT) as usize;
        let reserved_start = reserved.start_frame;
        let reserved_end = reserved.start_frame.saturating_add(reserved.frame_count);

        if reserved_end <= region_start_frame || reserved_start >= region_end_frame {
            self.add_frame_range(region_start_frame, region_end_frame);
            return;
        }

        if reserved_start > region_start_frame {
            self.add_frame_range(region_start_frame, reserved_start);
        }

        if reserved_end < region_end_frame {
            self.add_frame_range(reserved_end, region_end_frame);
        }
    }

    fn add_frame_range(&mut self, mut start_frame: usize, end_frame: usize) {
        while start_frame < end_frame {
            let remaining = end_frame - start_frame;
            let order = self.best_order(start_frame, remaining);
            self.push_free(start_frame, order);
            start_frame += 1usize << order;
        }
    }

    fn best_order(&self, start_frame: usize, remaining_frames: usize) -> usize {
        let mut order = usize::min(MAX_ORDER, floor_log2(remaining_frames));
        while order > 0 && (start_frame & ((1usize << order) - 1)) != 0 {
            order -= 1;
        }
        order
    }

    fn alloc_block(&mut self, order: usize) -> Result<usize, FrameAllocError> {
        if order > MAX_ORDER {
            return Err(FrameAllocError::InvalidCount);
        }

        let mut current_order = order;
        while current_order <= MAX_ORDER && self.free_heads[current_order] == NONE {
            current_order += 1;
        }

        if current_order > MAX_ORDER {
            return Err(FrameAllocError::OutOfMemory);
        }

        let block = self
            .pop_free(current_order)
            .expect("free list head must exist");
        while current_order > order {
            current_order -= 1;
            let buddy = block + (1usize << current_order);
            self.push_free(buddy, current_order);
        }

        let meta = &mut self.metadata[block];
        meta.ref_count = 1;
        meta.next = NONE;
        meta.order = order as u8;
        meta.state = FrameState::Allocated;
        Ok(block)
    }

    fn free_block(&mut self, mut block: usize, mut order: usize) {
        loop {
            if order >= MAX_ORDER {
                break;
            }

            let buddy = block ^ (1usize << order);
            if buddy >= self.total_frames || !self.is_free_block(buddy, order) {
                break;
            }

            self.remove_free(buddy, order);
            self.metadata[buddy] = FrameMeta::EMPTY;
            block = usize::min(block, buddy);
            order += 1;
        }

        self.push_free(block, order);
    }

    fn push_free(&mut self, block: usize, order: usize) {
        let meta = &mut self.metadata[block];
        meta.ref_count = 0;
        meta.order = order as u8;
        meta.state = FrameState::Free;
        meta.next = self.free_heads[order];
        self.free_heads[order] = block as u32;
    }

    fn pop_free(&mut self, order: usize) -> Option<usize> {
        let head = self.free_heads[order];
        if head == NONE {
            return None;
        }

        let block = head as usize;
        self.free_heads[order] = self.metadata[block].next;
        self.metadata[block].next = NONE;
        Some(block)
    }

    fn remove_free(&mut self, block: usize, order: usize) {
        let mut current = self.free_heads[order];
        let mut previous = NONE;

        while current != NONE {
            let current_index = current as usize;
            if current_index == block {
                let next = self.metadata[current_index].next;
                if previous == NONE {
                    self.free_heads[order] = next;
                } else {
                    self.metadata[previous as usize].next = next;
                }
                self.metadata[current_index].next = NONE;
                return;
            }

            previous = current;
            current = self.metadata[current_index].next;
        }
    }

    fn is_free_block(&self, block: usize, order: usize) -> bool {
        let meta = &self.metadata[block];
        matches!(meta.state, FrameState::Free) && meta.order as usize == order
    }

    const fn block_from_frame(&self, frame: PhysFrame) -> Result<usize, FrameAllocError> {
        let index = frame.index();
        if index >= self.total_frames {
            return Err(FrameAllocError::InvalidFrame);
        }
        Ok(index)
    }
}

impl FrameAllocator for BuddyFrameAllocator {
    fn alloc(&mut self, count: usize) -> Result<PhysFrame, FrameAllocError> {
        let order = order_for_count(count)?;
        let block = self.alloc_block(order)?;
        Ok(PhysFrame::from_start_address(PhysAddr::new(
            (block as u64) << PAGE_SHIFT,
        )))
    }

    fn retain(&mut self, frame: PhysFrame) -> Result<usize, FrameAllocError> {
        let block = self.block_from_frame(frame)?;
        let meta = &mut self.metadata[block];
        if !matches!(meta.state, FrameState::Allocated) {
            return Err(FrameAllocError::InvalidFrame);
        }
        meta.ref_count = meta
            .ref_count
            .checked_add(1)
            .ok_or(FrameAllocError::RefCountOverflow)?;
        Ok(meta.ref_count as usize)
    }

    fn release(&mut self, frame: PhysFrame, count: usize) -> Result<usize, FrameAllocError> {
        let block = self.block_from_frame(frame)?;
        let order = order_for_count(count)?;
        let meta = &mut self.metadata[block];
        if !matches!(meta.state, FrameState::Allocated) {
            return Err(FrameAllocError::InvalidFrame);
        }
        if meta.order as usize != order {
            return Err(FrameAllocError::InvalidFrame);
        }
        if meta.ref_count == 0 {
            return Err(FrameAllocError::RefCountUnderflow);
        }

        meta.ref_count -= 1;
        let remaining = meta.ref_count as usize;
        if remaining == 0 {
            self.free_block(block, order);
        }

        Ok(remaining)
    }

    fn ref_count(&self, frame: PhysFrame) -> Option<usize> {
        let block = self.block_from_frame(frame).ok()?;
        let meta = &self.metadata[block];
        matches!(meta.state, FrameState::Allocated).then_some(meta.ref_count as usize)
    }
}

fn reserve_metadata(memory_map: &MemoryMap<'_>, required_frames: usize) -> Option<PhysAddr> {
    memory_map
        .iter()
        .copied()
        .filter(|region| region.kind == MemoryRegionKind::USABLE)
        .find_map(|region| {
            let start = align_up(region.start, PAGE_SIZE);
            let end = align_down(region.start.saturating_add(region.len), PAGE_SIZE);
            let frames = ((end.saturating_sub(start)) >> PAGE_SHIFT) as usize;
            (frames >= required_frames).then_some(PhysAddr::new(start))
        })
}

const fn region_end(region: &MemoryRegion) -> u64 {
    region.start.saturating_add(region.len)
}

const fn align_up(value: u64, align: u64) -> u64 {
    (value + align - 1) & !(align - 1)
}

const fn align_down(value: u64, align: u64) -> u64 {
    value & !(align - 1)
}

const fn div_ceil(value: usize, divisor: usize) -> usize {
    value.div_ceil(divisor)
}

const fn order_for_count(count: usize) -> Result<usize, FrameAllocError> {
    if count == 0 {
        return Err(FrameAllocError::InvalidCount);
    }

    let order = ceil_log2(count);
    if order > MAX_ORDER {
        return Err(FrameAllocError::InvalidCount);
    }

    Ok(order)
}

const fn ceil_log2(value: usize) -> usize {
    if value <= 1 {
        0
    } else {
        usize::BITS as usize - (value - 1).leading_zeros() as usize
    }
}

const fn floor_log2(value: usize) -> usize {
    usize::BITS as usize - 1 - value.leading_zeros() as usize
}
