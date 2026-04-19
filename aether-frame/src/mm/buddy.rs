use core::mem::MaybeUninit;
use core::ptr;

use crate::boot::{MemoryMap, MemoryRegionKind};

use super::address::{PhysAddr, PAGE_SHIFT, PAGE_SIZE};
use super::frame::{FrameAllocError, FrameAllocator, PhysFrame};

const MAX_ORDER_SLOTS: usize = usize::BITS as usize;
const PER_CPU_CACHE_MAX_ORDER: usize = 3;
const DEFAULT_MAX_USABLE_RANGES: usize = 32;
const DEFAULT_MAX_CPUS: usize = 256;
const DEFAULT_MAX_BLOCK_NODES: usize = 262_144;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BlockNode {
    start: usize,
    order: usize,
    refcount: u32,
    next: Option<usize>,
}

impl BlockNode {
    const EMPTY: Self = Self {
        start: 0,
        order: 0,
        refcount: 0,
        next: None,
    };
}

#[derive(Debug, Clone)]
struct NodePool<const CAP: usize> {
    nodes: [BlockNode; CAP],
    unused_head: Option<usize>,
    unused_len: usize,
}

impl<const CAP: usize> NodePool<CAP> {
    fn has_unused(&self, need: usize) -> bool {
        self.unused_len >= need
    }

    fn alloc_unused(&mut self) -> Result<usize, FrameAllocError> {
        let index = self.unused_head.ok_or(FrameAllocError::MetadataExhausted)?;
        self.unused_head = self.nodes[index].next;
        self.nodes[index].next = None;
        self.unused_len -= 1;
        Ok(index)
    }

    fn release_unused(&mut self, index: usize) {
        self.nodes[index] = BlockNode {
            next: self.unused_head,
            ..BlockNode::EMPTY
        };
        self.unused_head = Some(index);
        self.unused_len += 1;
    }

    unsafe fn init_in_place(out: *mut Self) {
        let nodes = ptr::addr_of_mut!((*out).nodes).cast::<BlockNode>();
        let mut index = 0usize;
        while index < CAP {
            nodes.add(index).write(BlockNode {
                next: if index + 1 < CAP {
                    Some(index + 1)
                } else {
                    None
                },
                ..BlockNode::EMPTY
            });
            index += 1;
        }

        ptr::addr_of_mut!((*out).unused_head).write(if CAP == 0 { None } else { Some(0) });
        ptr::addr_of_mut!((*out).unused_len).write(CAP);
    }
}

#[derive(Debug, Clone, Copy)]
struct RangeList<const CAP: usize> {
    entries: [(usize, usize); CAP],
    len: usize,
}

impl<const CAP: usize> RangeList<CAP> {
    fn new() -> Self {
        Self {
            entries: [(0, 0); CAP],
            len: 0,
        }
    }

    fn clear(&mut self) {
        self.len = 0;
    }

    fn len(&self) -> usize {
        self.len
    }

    fn get(&self, index: usize) -> (usize, usize) {
        self.entries[index]
    }

    fn as_slice(&self) -> &[(usize, usize)] {
        &self.entries[..self.len]
    }

    fn total_frames(&self) -> Result<usize, FrameAllocError> {
        let mut total = 0usize;
        let mut index = 0;
        while index < self.len {
            total = total
                .checked_add(self.entries[index].1)
                .ok_or(FrameAllocError::InvalidMemoryMap)?;
            index += 1;
        }
        Ok(total)
    }

    fn insert_merged_sorted(
        &mut self,
        mut start: usize,
        mut len: usize,
    ) -> Result<(), FrameAllocError> {
        if len == 0 {
            return Ok(());
        }

        let mut index = 0;
        while index < self.len && self.entries[index].0 < start {
            index += 1;
        }

        if index > 0 {
            let (prev_start, prev_len) = self.entries[index - 1];
            let prev_end = prev_start
                .checked_add(prev_len)
                .ok_or(FrameAllocError::InvalidMemoryMap)?;
            if start < prev_end {
                return Err(FrameAllocError::InvalidMemoryMap);
            }
            if start == prev_end {
                start = prev_start;
                len = prev_len
                    .checked_add(len)
                    .ok_or(FrameAllocError::InvalidMemoryMap)?;
                self.remove_at(index - 1);
                index -= 1;
            }
        }

        while index < self.len {
            let (next_start, next_len) = self.entries[index];
            let end = start
                .checked_add(len)
                .ok_or(FrameAllocError::InvalidMemoryMap)?;
            if next_start < end {
                return Err(FrameAllocError::InvalidMemoryMap);
            }
            if next_start != end {
                break;
            }
            len = len
                .checked_add(next_len)
                .ok_or(FrameAllocError::InvalidMemoryMap)?;
            self.remove_at(index);
        }

        self.insert_at(index, (start, len))
    }

    fn insert_at(&mut self, index: usize, entry: (usize, usize)) -> Result<(), FrameAllocError> {
        if self.len == CAP {
            return Err(FrameAllocError::MetadataExhausted);
        }

        let mut cursor = self.len;
        while cursor > index {
            self.entries[cursor] = self.entries[cursor - 1];
            cursor -= 1;
        }
        self.entries[index] = entry;
        self.len += 1;
        Ok(())
    }

    fn remove_at(&mut self, index: usize) {
        let mut cursor = index + 1;
        while cursor < self.len {
            self.entries[cursor - 1] = self.entries[cursor];
            cursor += 1;
        }
        self.len -= 1;
    }
}

#[repr(C, align(64))]
#[derive(Debug, Clone, Copy)]
struct PerCpuCache {
    heads: [Option<usize>; PER_CPU_CACHE_MAX_ORDER + 1],
    lens: [usize; PER_CPU_CACHE_MAX_ORDER + 1],
}

impl PerCpuCache {
    const fn new() -> Self {
        Self {
            heads: [None; PER_CPU_CACHE_MAX_ORDER + 1],
            lens: [0; PER_CPU_CACHE_MAX_ORDER + 1],
        }
    }

    fn len(&self, order: usize) -> usize {
        self.lens[order]
    }

    fn free_pages(&self) -> usize {
        let mut total = 0usize;
        let mut order = 0;
        while order <= PER_CPU_CACHE_MAX_ORDER {
            total += self.lens[order] * block_size(order);
            order += 1;
        }
        total
    }
}

#[derive(Debug, Clone)]
pub struct BuddyAllocator<
    const MAX_USABLE_RANGES: usize = DEFAULT_MAX_USABLE_RANGES,
    const MAX_CPUS: usize = DEFAULT_MAX_CPUS,
    const MAX_BLOCK_NODES: usize = DEFAULT_MAX_BLOCK_NODES,
> {
    total_frames: usize,
    max_order: usize,
    cpu_count: usize,
    usable_ranges: RangeList<MAX_USABLE_RANGES>,
    global_free_heads: [Option<usize>; MAX_ORDER_SLOTS],
    global_free_lens: [usize; MAX_ORDER_SLOTS],
    allocations_head: Option<usize>,
    allocation_len: usize,
    per_cpu: [PerCpuCache; MAX_CPUS],
    nodes: NodePool<MAX_BLOCK_NODES>,
}

impl<const MAX_USABLE_RANGES: usize, const MAX_CPUS: usize, const MAX_BLOCK_NODES: usize>
    BuddyAllocator<MAX_USABLE_RANGES, MAX_CPUS, MAX_BLOCK_NODES>
{
    pub fn bootstrap(mem_map: &MemoryMap, cpu_count: usize) -> Result<Self, FrameAllocError> {
        let mut allocator = MaybeUninit::<Self>::uninit();
        unsafe {
            Self::bootstrap_in_place(allocator.as_mut_ptr(), mem_map, cpu_count)?;
            Ok(allocator.assume_init())
        }
    }

    pub unsafe fn bootstrap_in_place(
        out: *mut Self,
        mem_map: &MemoryMap,
        cpu_count: usize,
    ) -> Result<(), FrameAllocError> {
        if cpu_count == 0 || cpu_count > MAX_CPUS {
            return Err(FrameAllocError::InvalidCount);
        }

        let mut usable_ranges = RangeList::new();
        collect_usable_ranges(mem_map, &mut usable_ranges)?;
        unsafe { Self::init_from_usable_ranges_in_place(out, usable_ranges, cpu_count) }
    }

    unsafe fn init_from_usable_ranges_in_place(
        out: *mut Self,
        usable_ranges: RangeList<MAX_USABLE_RANGES>,
        cpu_count: usize,
    ) -> Result<(), FrameAllocError> {
        if cpu_count == 0 || cpu_count > MAX_CPUS {
            return Err(FrameAllocError::InvalidCount);
        }

        let total_frames = usable_ranges.total_frames()?;
        if total_frames == 0 {
            return Err(FrameAllocError::InvalidMemoryMap);
        }

        let max_order = floor_log2(total_frames);
        if max_order >= MAX_ORDER_SLOTS {
            return Err(FrameAllocError::InvalidCount);
        }

        unsafe {
            ptr::addr_of_mut!((*out).total_frames).write(total_frames);
            ptr::addr_of_mut!((*out).max_order).write(max_order);
            ptr::addr_of_mut!((*out).cpu_count).write(cpu_count);
            ptr::addr_of_mut!((*out).usable_ranges).write(usable_ranges);
            initialize_array(
                ptr::addr_of_mut!((*out).global_free_heads).cast::<Option<usize>>(),
                MAX_ORDER_SLOTS,
                None,
            );
            initialize_array(
                ptr::addr_of_mut!((*out).global_free_lens).cast::<usize>(),
                MAX_ORDER_SLOTS,
                0,
            );
            ptr::addr_of_mut!((*out).allocations_head).write(None);
            ptr::addr_of_mut!((*out).allocation_len).write(0);
            initialize_array(
                ptr::addr_of_mut!((*out).per_cpu).cast::<PerCpuCache>(),
                MAX_CPUS,
                PerCpuCache::new(),
            );
            NodePool::<MAX_BLOCK_NODES>::init_in_place(ptr::addr_of_mut!((*out).nodes));
        }

        let allocator = unsafe { &mut *out };

        let mut index = 0;
        while index < allocator.usable_ranges.len() {
            let (start_frame, frame_count) = allocator.usable_ranges.get(index);
            allocator.seed_global_free_region(start_frame, frame_count)?;
            index += 1;
        }

        Ok(())
    }

    pub fn cpu_count(&self) -> usize {
        self.cpu_count
    }

    pub fn total_frames(&self) -> usize {
        self.total_frames
    }

    pub fn free_pages(&self) -> usize {
        let mut global = 0usize;
        let mut order = 0usize;
        while order <= self.max_order {
            global += self.global_free_lens[order] * block_size(order);
            order += 1;
        }

        let mut local = 0usize;
        let mut cpu_id = 0usize;
        while cpu_id < self.cpu_count {
            local += self.per_cpu[cpu_id].free_pages();
            cpu_id += 1;
        }

        global + local
    }

    #[allow(non_snake_case)]
    pub fn alloc_frame_4K(&mut self, count: usize) -> Result<u64, FrameAllocError> {
        self.alloc_frame_4k_on_cpu(0, count)
    }

    pub fn alloc_frame_4k_on_cpu(
        &mut self,
        cpu_id: usize,
        count: usize,
    ) -> Result<u64, FrameAllocError> {
        let order = order_for_count(count, self.max_order)?;
        let node_index = self.alloc_block(cpu_id, order)?;
        let block = self.mark_allocated(node_index, order)?;
        Ok((block as u64) << PAGE_SHIFT)
    }

    pub fn retain_frame(&mut self, block: u64) -> Result<u32, FrameAllocError> {
        let frame = self.frame_number_from_addr(block)?;
        let node_index = self
            .find_allocation_index(frame)
            .ok_or(FrameAllocError::InvalidFrame)?;
        let node = &mut self.nodes.nodes[node_index];
        node.refcount = node
            .refcount
            .checked_add(1)
            .ok_or(FrameAllocError::RefCountOverflow)?;
        Ok(node.refcount)
    }

    pub fn free_frame(&mut self, block: u64) -> Result<(), FrameAllocError> {
        self.free_frame_on_cpu(0, block)
    }

    pub fn free_frame_on_cpu(&mut self, cpu_id: usize, block: u64) -> Result<(), FrameAllocError> {
        let cpu_id = self.checked_cpu(cpu_id)?;
        let frame = self.frame_number_from_addr(block)?;
        let node_index = self
            .find_allocation_index(frame)
            .ok_or(FrameAllocError::InvalidFrame)?;

        let should_release = {
            let node = &mut self.nodes.nodes[node_index];
            node.refcount = node
                .refcount
                .checked_sub(1)
                .ok_or(FrameAllocError::RefCountUnderflow)?;
            node.refcount == 0
        };

        if !should_release {
            return Ok(());
        }

        let order = self.nodes.nodes[node_index].order;
        self.remove_allocation_node(node_index)?;

        if order <= PER_CPU_CACHE_MAX_ORDER {
            self.cpu_push(cpu_id, order, node_index);
            self.drain_per_cpu_cache(cpu_id, order)?;
        } else {
            self.free_global_block(node_index)?;
        }

        Ok(())
    }

    pub fn cpu_cache_len(&self, cpu_id: usize, order: usize) -> Result<usize, FrameAllocError> {
        let cpu_id = self.checked_cpu(cpu_id)?;
        if order > PER_CPU_CACHE_MAX_ORDER {
            return Err(FrameAllocError::InvalidCount);
        }
        Ok(self.per_cpu[cpu_id].len(order))
    }

    pub fn global_free_len(&self, order: usize) -> Result<usize, FrameAllocError> {
        if order > self.max_order {
            return Err(FrameAllocError::InvalidCount);
        }
        Ok(self.global_free_lens[order])
    }

    pub fn ref_count_for_addr(&self, block: u64) -> Option<usize> {
        let frame = self.frame_number_from_addr(block).ok()?;
        let node_index = self.find_allocation_index(frame)?;
        Some(self.nodes.nodes[node_index].refcount as usize)
    }

    pub fn validate_invariants(&self) -> Result<(), &'static str> {
        if self.max_order >= MAX_ORDER_SLOTS {
            return Err("max_order exceeds supported order slots");
        }

        let mut visited = [false; MAX_BLOCK_NODES];
        let mut starts = [0usize; MAX_BLOCK_NODES];
        let mut lens = [0usize; MAX_BLOCK_NODES];
        let mut block_count = 0usize;
        let mut accounted_pages = 0usize;

        let mut allocation_count = 0usize;
        let mut allocation_cursor = self.allocations_head;
        while let Some(node_index) = allocation_cursor {
            let node = self.validate_node_index(node_index, &visited)?;
            if node.refcount == 0 {
                return Err("allocated block has zero refcount");
            }
            if node.order > self.max_order {
                return Err("allocated block order exceeds max_order");
            }

            let len = block_size(node.order);
            validate_block_shape(node.start, len, self.usable_ranges.as_slice())?;
            visited[node_index] = true;
            starts[block_count] = node.start;
            lens[block_count] = len;
            block_count += 1;
            accounted_pages = accounted_pages
                .checked_add(len)
                .ok_or("allocated page accounting overflow")?;
            allocation_count += 1;
            allocation_cursor = node.next;
        }

        if allocation_count != self.allocation_len {
            return Err("allocation list length mismatch");
        }

        let mut order = 0usize;
        while order < MAX_ORDER_SLOTS {
            let mut list_len = 0usize;
            let mut cursor = self.global_free_heads[order];
            while let Some(node_index) = cursor {
                let node = self.validate_node_index(node_index, &visited)?;
                if order > self.max_order {
                    return Err("free block stored beyond max_order");
                }
                if node.refcount != 0 {
                    return Err("global free block has non-zero refcount");
                }
                if node.order != order {
                    return Err("global free block stored in wrong order list");
                }

                let len = block_size(order);
                validate_block_shape(node.start, len, self.usable_ranges.as_slice())?;
                visited[node_index] = true;
                starts[block_count] = node.start;
                lens[block_count] = len;
                block_count += 1;
                accounted_pages = accounted_pages
                    .checked_add(len)
                    .ok_or("global free page accounting overflow")?;
                list_len += 1;
                cursor = node.next;
            }

            if list_len != self.global_free_lens[order] {
                return Err("global free list length mismatch");
            }
            order += 1;
        }

        let mut cpu_id = 0usize;
        while cpu_id < MAX_CPUS {
            let mut order = 0usize;
            while order <= PER_CPU_CACHE_MAX_ORDER {
                let mut list_len = 0usize;
                let mut cursor = self.per_cpu[cpu_id].heads[order];
                while let Some(node_index) = cursor {
                    let node = self.validate_node_index(node_index, &visited)?;
                    if cpu_id >= self.cpu_count {
                        return Err("inactive cpu cache contains free blocks");
                    }
                    if node.refcount != 0 {
                        return Err("per-cpu free block has non-zero refcount");
                    }
                    if node.order != order {
                        return Err("per-cpu free block stored in wrong order list");
                    }

                    let len = block_size(order);
                    validate_block_shape(node.start, len, self.usable_ranges.as_slice())?;
                    visited[node_index] = true;
                    starts[block_count] = node.start;
                    lens[block_count] = len;
                    block_count += 1;
                    accounted_pages = accounted_pages
                        .checked_add(len)
                        .ok_or("per-cpu free page accounting overflow")?;
                    list_len += 1;
                    cursor = node.next;
                }

                if list_len != self.per_cpu[cpu_id].lens[order] {
                    return Err("per-cpu free list length mismatch");
                }
                order += 1;
            }
            cpu_id += 1;
        }

        let mut left = 0usize;
        while left < block_count {
            let left_end = starts[left]
                .checked_add(lens[left])
                .ok_or("block end overflow")?;
            let mut right = left + 1;
            while right < block_count {
                let right_end = starts[right]
                    .checked_add(lens[right])
                    .ok_or("block end overflow")?;
                if starts[left] < right_end && starts[right] < left_end {
                    return Err("allocator blocks overlap");
                }
                right += 1;
            }
            left += 1;
        }

        if accounted_pages != self.total_frames {
            return Err("allocator page accounting mismatch");
        }

        let mut unused_count = 0usize;
        let mut unused_cursor = self.nodes.unused_head;
        while let Some(node_index) = unused_cursor {
            if node_index >= MAX_BLOCK_NODES {
                return Err("node index out of bounds");
            }
            if visited[node_index] {
                return Err("metadata node appears in both active and unused lists");
            }
            visited[node_index] = true;
            unused_count += 1;
            unused_cursor = self.nodes.nodes[node_index].next;
        }

        if unused_count != self.nodes.unused_len {
            return Err("unused node count mismatch");
        }

        let mut node_index = 0usize;
        while node_index < MAX_BLOCK_NODES {
            if !visited[node_index] {
                return Err("metadata node leaked from all lists");
            }
            node_index += 1;
        }

        Ok(())
    }

    fn alloc_block(&mut self, cpu_id: usize, order: usize) -> Result<usize, FrameAllocError> {
        let cpu_id = self.checked_cpu(cpu_id)?;
        if order <= PER_CPU_CACHE_MAX_ORDER {
            if let Some(node_index) = self.cpu_pop(cpu_id, order) {
                return Ok(node_index);
            }
            self.refill_per_cpu_cache(cpu_id, order)?;
            if let Some(node_index) = self.cpu_pop(cpu_id, order) {
                return Ok(node_index);
            }
        }
        self.alloc_from_global(order)
    }

    fn refill_per_cpu_cache(&mut self, cpu_id: usize, order: usize) -> Result<(), FrameAllocError> {
        let target = refill_batch(order);
        let mut refill_count = 0usize;
        while refill_count < target {
            match self.alloc_from_global(order) {
                Ok(node_index) => {
                    self.cpu_push(cpu_id, order, node_index);
                    refill_count += 1;
                }
                Err(FrameAllocError::OutOfMemory) => break,
                Err(err) => return Err(err),
            }
        }

        if self.per_cpu[cpu_id].len(order) == 0 {
            return Err(FrameAllocError::OutOfMemory);
        }
        Ok(())
    }

    fn drain_per_cpu_cache(&mut self, cpu_id: usize, order: usize) -> Result<(), FrameAllocError> {
        let low = refill_batch(order);
        let high = high_watermark(order);
        if self.per_cpu[cpu_id].len(order) <= high {
            return Ok(());
        }

        while self.per_cpu[cpu_id].len(order) > low {
            let node_index = self
                .cpu_pop(cpu_id, order)
                .expect("cache length was checked before draining");
            self.free_global_block(node_index)?;
        }

        Ok(())
    }

    fn checked_cpu(&self, cpu_id: usize) -> Result<usize, FrameAllocError> {
        if cpu_id < self.cpu_count {
            Ok(cpu_id)
        } else {
            Err(FrameAllocError::InvalidCpu)
        }
    }

    fn frame_number_from_addr(&self, block: u64) -> Result<usize, FrameAllocError> {
        if block & (PAGE_SIZE - 1) != 0 {
            return Err(FrameAllocError::InvalidFrame);
        }
        Ok((block as usize) >> PAGE_SHIFT)
    }

    fn checked_addr_from_frame(frame: PhysFrame) -> Result<u64, FrameAllocError> {
        let addr = frame.start_address();
        if !addr.is_aligned(PAGE_SIZE) {
            return Err(FrameAllocError::InvalidFrame);
        }
        Ok(addr.as_u64())
    }

    fn frame_from_addr(addr: u64) -> PhysFrame {
        PhysFrame::from_start_address(PhysAddr::new(addr))
    }

    fn ensure_release_matches_count(
        &self,
        frame: PhysFrame,
        count: usize,
    ) -> Result<usize, FrameAllocError> {
        let order = order_for_count(count, self.max_order)?;
        let node_index = self
            .find_allocation_index(frame.index())
            .ok_or(FrameAllocError::InvalidFrame)?;
        let node = self.nodes.nodes[node_index];
        if node.order != order {
            return Err(FrameAllocError::InvalidCount);
        }
        Ok(node.refcount as usize)
    }

    fn seed_global_free_region(
        &mut self,
        mut start: usize,
        mut remaining: usize,
    ) -> Result<(), FrameAllocError> {
        while remaining > 0 {
            let order = max_fit_order(start, remaining, self.max_order);
            let node_index = self.nodes.alloc_unused()?;
            self.nodes.nodes[node_index] = BlockNode {
                start,
                order,
                refcount: 0,
                next: None,
            };
            self.global_insert_sorted(order, node_index)?;
            start += block_size(order);
            remaining -= block_size(order);
        }
        Ok(())
    }

    fn alloc_from_global(&mut self, order: usize) -> Result<usize, FrameAllocError> {
        let mut current_order = order;
        while current_order <= self.max_order && self.global_free_heads[current_order].is_none() {
            current_order += 1;
        }

        if current_order > self.max_order {
            return Err(FrameAllocError::OutOfMemory);
        }

        let extra_nodes = current_order - order;
        if !self.nodes.has_unused(extra_nodes) {
            return Err(FrameAllocError::MetadataExhausted);
        }

        let node_index = self
            .global_pop_first(current_order)
            .expect("free list cannot be empty after successful search");
        self.nodes.nodes[node_index].order = current_order;

        while current_order > order {
            current_order -= 1;
            let buddy_start = self.nodes.nodes[node_index].start + block_size(current_order);
            let buddy_index = self.nodes.alloc_unused()?;
            self.nodes.nodes[buddy_index] = BlockNode {
                start: buddy_start,
                order: current_order,
                refcount: 0,
                next: None,
            };
            self.global_insert_sorted(current_order, buddy_index)?;
            self.nodes.nodes[node_index].order = current_order;
        }

        Ok(node_index)
    }

    fn free_global_block(&mut self, node_index: usize) -> Result<(), FrameAllocError> {
        let mut block = self.nodes.nodes[node_index].start;
        let mut order = self.nodes.nodes[node_index].order;

        while order < self.max_order {
            let buddy = block ^ block_size(order);
            if let Some(buddy_index) = self.global_remove_by_start(order, buddy) {
                self.nodes.release_unused(buddy_index);
                block = block.min(buddy);
                order += 1;
            } else {
                break;
            }
        }

        self.nodes.nodes[node_index].start = block;
        self.nodes.nodes[node_index].order = order;
        self.nodes.nodes[node_index].refcount = 0;
        self.nodes.nodes[node_index].next = None;
        self.global_insert_sorted(order, node_index)
    }

    fn mark_allocated(
        &mut self,
        node_index: usize,
        order: usize,
    ) -> Result<usize, FrameAllocError> {
        let node = &mut self.nodes.nodes[node_index];
        if node.refcount != 0 {
            return Err(FrameAllocError::InvalidFrame);
        }

        node.order = order;
        node.refcount = 1;
        node.next = self.allocations_head;
        self.allocations_head = Some(node_index);
        self.allocation_len += 1;
        Ok(node.start)
    }

    fn find_allocation_index(&self, start: usize) -> Option<usize> {
        let mut cursor = self.allocations_head;
        while let Some(node_index) = cursor {
            let node = self.nodes.nodes[node_index];
            if node.start == start {
                return Some(node_index);
            }
            cursor = node.next;
        }
        None
    }

    fn remove_allocation_node(&mut self, target: usize) -> Result<(), FrameAllocError> {
        let mut previous: Option<usize> = None;
        let mut cursor = self.allocations_head;

        while let Some(node_index) = cursor {
            if node_index == target {
                let next = self.nodes.nodes[node_index].next;
                if let Some(previous_index) = previous {
                    self.nodes.nodes[previous_index].next = next;
                } else {
                    self.allocations_head = next;
                }
                self.nodes.nodes[node_index].next = None;
                self.allocation_len -= 1;
                return Ok(());
            }
            previous = Some(node_index);
            cursor = self.nodes.nodes[node_index].next;
        }

        Err(FrameAllocError::InvalidFrame)
    }

    fn cpu_push(&mut self, cpu_id: usize, order: usize, node_index: usize) {
        self.nodes.nodes[node_index].order = order;
        self.nodes.nodes[node_index].refcount = 0;
        self.nodes.nodes[node_index].next = self.per_cpu[cpu_id].heads[order];
        self.per_cpu[cpu_id].heads[order] = Some(node_index);
        self.per_cpu[cpu_id].lens[order] += 1;
    }

    fn cpu_pop(&mut self, cpu_id: usize, order: usize) -> Option<usize> {
        let node_index = self.per_cpu[cpu_id].heads[order]?;
        self.per_cpu[cpu_id].heads[order] = self.nodes.nodes[node_index].next;
        self.per_cpu[cpu_id].lens[order] -= 1;
        self.nodes.nodes[node_index].next = None;
        Some(node_index)
    }

    fn global_insert_sorted(
        &mut self,
        order: usize,
        node_index: usize,
    ) -> Result<(), FrameAllocError> {
        let start = self.nodes.nodes[node_index].start;
        let mut previous: Option<usize> = None;
        let mut cursor = self.global_free_heads[order];

        while let Some(current_index) = cursor {
            let current_start = self.nodes.nodes[current_index].start;
            if current_start == start {
                return Err(FrameAllocError::InvalidFrame);
            }
            if current_start > start {
                break;
            }
            previous = Some(current_index);
            cursor = self.nodes.nodes[current_index].next;
        }

        self.nodes.nodes[node_index].next = cursor;
        if let Some(previous_index) = previous {
            self.nodes.nodes[previous_index].next = Some(node_index);
        } else {
            self.global_free_heads[order] = Some(node_index);
        }
        self.global_free_lens[order] += 1;
        Ok(())
    }

    fn global_pop_first(&mut self, order: usize) -> Option<usize> {
        let node_index = self.global_free_heads[order]?;
        self.global_free_heads[order] = self.nodes.nodes[node_index].next;
        self.global_free_lens[order] -= 1;
        self.nodes.nodes[node_index].next = None;
        Some(node_index)
    }

    fn global_remove_by_start(&mut self, order: usize, start: usize) -> Option<usize> {
        let mut previous: Option<usize> = None;
        let mut cursor = self.global_free_heads[order];

        while let Some(node_index) = cursor {
            let current_start = self.nodes.nodes[node_index].start;
            if current_start == start {
                let next = self.nodes.nodes[node_index].next;
                if let Some(previous_index) = previous {
                    self.nodes.nodes[previous_index].next = next;
                } else {
                    self.global_free_heads[order] = next;
                }
                self.global_free_lens[order] -= 1;
                self.nodes.nodes[node_index].next = None;
                return Some(node_index);
            }
            if current_start > start {
                return None;
            }
            previous = Some(node_index);
            cursor = self.nodes.nodes[node_index].next;
        }

        None
    }

    fn validate_node_index(
        &self,
        node_index: usize,
        visited: &[bool; MAX_BLOCK_NODES],
    ) -> Result<BlockNode, &'static str> {
        if node_index >= MAX_BLOCK_NODES {
            return Err("node index out of bounds");
        }
        if visited[node_index] {
            return Err("metadata node appears in multiple lists");
        }
        Ok(self.nodes.nodes[node_index])
    }
}

impl<const MAX_USABLE_RANGES: usize, const MAX_CPUS: usize, const MAX_BLOCK_NODES: usize>
    FrameAllocator for BuddyAllocator<MAX_USABLE_RANGES, MAX_CPUS, MAX_BLOCK_NODES>
{
    fn alloc(&mut self, count: usize) -> Result<PhysFrame, FrameAllocError> {
        let addr = self.alloc_frame_4K(count)?;
        Ok(Self::frame_from_addr(addr))
    }

    fn retain(&mut self, frame: PhysFrame) -> Result<usize, FrameAllocError> {
        self.retain_frame(Self::checked_addr_from_frame(frame)?)
            .map(|count| count as usize)
    }

    fn release(&mut self, frame: PhysFrame, count: usize) -> Result<usize, FrameAllocError> {
        Self::checked_addr_from_frame(frame)?;
        let current = self.ensure_release_matches_count(frame, count)?;
        self.free_frame(Self::checked_addr_from_frame(frame)?)?;
        Ok(current - 1)
    }

    fn ref_count(&self, frame: PhysFrame) -> Option<usize> {
        let addr = Self::checked_addr_from_frame(frame).ok()?;
        self.ref_count_for_addr(addr)
    }
}

fn collect_usable_ranges<const CAP: usize>(
    mem_map: &MemoryMap,
    ranges: &mut RangeList<CAP>,
) -> Result<(), FrameAllocError> {
    ranges.clear();

    for region in mem_map.iter() {
        if region.kind != MemoryRegionKind::USABLE {
            continue;
        }

        let region_start =
            usize::try_from(region.start).map_err(|_| FrameAllocError::InvalidMemoryMap)?;
        let region_end = region
            .start
            .checked_add(region.len)
            .and_then(|end| usize::try_from(end).ok())
            .ok_or(FrameAllocError::InvalidMemoryMap)?;

        let start = align_up(region_start, PAGE_SIZE as usize);
        let end = align_down(region_end, PAGE_SIZE as usize);
        if end <= start {
            continue;
        }

        let start_frame = start >> PAGE_SHIFT;
        let frame_count = (end - start) >> PAGE_SHIFT;
        ranges.insert_merged_sorted(start_frame, frame_count)?;
    }

    Ok(())
}

fn validate_block_shape(
    start: usize,
    len: usize,
    usable_ranges: &[(usize, usize)],
) -> Result<(), &'static str> {
    if len == 0 {
        return Err("block length cannot be zero");
    }
    if start % len != 0 {
        return Err("block is not aligned to its order");
    }
    if block_fits_usable_ranges(start, len, usable_ranges) {
        Ok(())
    } else {
        Err("block falls outside usable ranges")
    }
}

fn block_fits_usable_ranges(start: usize, len: usize, usable_ranges: &[(usize, usize)]) -> bool {
    let Some(end) = start.checked_add(len) else {
        return false;
    };

    let mut index = 0usize;
    while index < usable_ranges.len() {
        let (region_start, region_len) = usable_ranges[index];
        let Some(region_end) = region_start.checked_add(region_len) else {
            return false;
        };
        if start >= region_start && end <= region_end {
            return true;
        }
        index += 1;
    }

    false
}

const fn order_for_count(count: usize, max_order: usize) -> Result<usize, FrameAllocError> {
    if count == 0 {
        return Err(FrameAllocError::InvalidCount);
    }
    let order = ceil_log2(count);
    if order > max_order {
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

const fn block_size(order: usize) -> usize {
    1usize << order
}

unsafe fn initialize_array<T: Copy>(out: *mut T, len: usize, value: T) {
    let mut index = 0usize;
    while index < len {
        unsafe {
            out.add(index).write(value);
        }
        index += 1;
    }
}

const fn align_up(value: usize, align: usize) -> usize {
    (value + align - 1) & !(align - 1)
}

const fn align_down(value: usize, align: usize) -> usize {
    value & !(align - 1)
}

const fn refill_batch(order: usize) -> usize {
    1usize << PER_CPU_CACHE_MAX_ORDER.saturating_sub(order)
}

const fn high_watermark(order: usize) -> usize {
    refill_batch(order) * 2
}

fn max_fit_order(start: usize, remaining: usize, max_order: usize) -> usize {
    let mut order = floor_log2(remaining).min(max_order);
    while order > 0 && start % block_size(order) != 0 {
        order -= 1;
    }
    order
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::boot::{MemoryMap, MemoryRegion, MemoryRegionKind};

    #[test]
    fn bootstrap_converts_usable_regions_from_bytes_to_frames() {
        let regions = [
            MemoryRegion {
                start: 0x0000,
                len: 0x1000,
                kind: MemoryRegionKind::RESERVED,
            },
            MemoryRegion {
                start: 0x1234,
                len: 0x6000,
                kind: MemoryRegionKind::USABLE,
            },
            MemoryRegion {
                start: 0x7000,
                len: 0x1000,
                kind: MemoryRegionKind::USABLE,
            },
            MemoryRegion {
                start: 0x9800,
                len: 0x2000,
                kind: MemoryRegionKind::USABLE,
            },
        ];

        let allocator =
            BuddyAllocator::<8, 4, 64>::bootstrap(&MemoryMap::new(&regions), 1).unwrap();

        assert_eq!(allocator.total_frames(), 7);
        assert_eq!(allocator.free_pages(), 7);
        assert!(allocator.validate_invariants().is_ok());
    }

    #[test]
    fn alloc_frame_uses_physical_frame_numbers_not_byte_addresses() {
        let regions = [MemoryRegion {
            start: 0x1000,
            len: 0x3000,
            kind: MemoryRegionKind::USABLE,
        }];
        let mut allocator =
            BuddyAllocator::<4, 4, 32>::bootstrap(&MemoryMap::new(&regions), 1).unwrap();

        let frame = allocator.alloc_frame_4K(1).unwrap();

        assert_eq!(frame, 0x1000);
        assert_eq!(allocator.ref_count_for_addr(frame), Some(1));
        assert!(allocator.validate_invariants().is_ok());
    }
}
