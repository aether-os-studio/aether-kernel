use core::marker::PhantomData;
use core::ptr;

use crate::boot::phys_to_virt;

use super::address::{PAGE_SIZE, VirtAddr};
use super::frame::{FrameAllocError, FrameAllocator, PhysFrame};

const MAX_PAGE_TABLE_LEVELS: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MapFlags(u64);

impl MapFlags {
    pub const READ: Self = Self(1 << 0);
    pub const WRITE: Self = Self(1 << 1);
    pub const EXECUTE: Self = Self(1 << 2);
    pub const USER: Self = Self(1 << 3);
    pub const GLOBAL: Self = Self(1 << 4);
    pub const NO_CACHE: Self = Self(1 << 5);
    pub const WRITE_THROUGH: Self = Self(1 << 6);

    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }

    #[must_use]
    pub const fn from_bits(bits: u64) -> Self {
        Self(bits)
    }

    #[must_use]
    pub const fn bits(self) -> u64 {
        self.0
    }

    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    #[must_use]
    pub const fn without(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }
}

impl core::ops::BitOr for MapFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapSize {
    Size4KiB,
    Size2MiB,
    Size1GiB,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MappingError {
    InvalidAddress,
    AlreadyMapped,
    NotMapped,
    UnsupportedSize,
    OutOfMemory,
    Frame(FrameAllocError),
}

impl From<FrameAllocError> for MappingError {
    fn from(value: FrameAllocError) -> Self {
        Self::Frame(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnmapResult {
    pub frame: PhysFrame,
    pub remaining_refs: usize,
    pub size: MapSize,
}

pub trait PageTableArch {
    const LEVELS: usize;
    const ENTRY_COUNT: usize;

    fn root_frame() -> PhysFrame;
    fn page_size(level: usize) -> Option<u64>;
    fn leaf_level(size: MapSize) -> Option<usize>;
    fn index_of(addr: VirtAddr, level: usize) -> usize;
    fn table_entry(frame: PhysFrame, index: usize) -> *mut u64;
    fn is_present(entry: u64) -> bool;
    fn is_leaf(entry: u64, level: usize) -> bool;
    fn entry_frame(entry: u64) -> PhysFrame;
    fn make_table(frame: PhysFrame, flags: MapFlags) -> u64;
    fn make_leaf(frame: PhysFrame, level: usize, flags: MapFlags) -> u64;
    fn invalidate(addr: VirtAddr);
    fn invalidate_all();
}

#[derive(Clone, Copy)]
struct WalkEntry {
    table: PhysFrame,
    index: usize,
}

pub struct AddressSpace<A: PageTableArch> {
    root: PhysFrame,
    _marker: PhantomData<A>,
}

impl<A: PageTableArch> AddressSpace<A> {
    #[must_use]
    pub fn current() -> Self {
        Self {
            root: A::root_frame(),
            _marker: PhantomData,
        }
    }

    #[must_use]
    pub const fn from_root(root: PhysFrame) -> Self {
        Self {
            root,
            _marker: PhantomData,
        }
    }

    #[must_use]
    pub const fn root(&self) -> PhysFrame {
        self.root
    }

    pub fn new_root<F: FrameAllocator>(allocator: &mut F) -> Result<Self, MappingError> {
        let root = allocator.alloc(1)?;
        unsafe { zero_page(root) };
        Ok(Self::from_root(root))
    }

    pub fn map<F: FrameAllocator>(
        &mut self,
        virt: VirtAddr,
        frame: PhysFrame,
        size: MapSize,
        flags: MapFlags,
        allocator: &mut F,
    ) -> Result<(), MappingError> {
        let leaf_level = A::leaf_level(size).ok_or(MappingError::UnsupportedSize)?;
        let page_size = A::page_size(leaf_level).ok_or(MappingError::UnsupportedSize)?;
        if !virt.is_aligned(page_size) || !frame.start_address().is_aligned(page_size) {
            return Err(MappingError::InvalidAddress);
        }

        let mut table = self.root;
        for level in 0..leaf_level {
            let index = A::index_of(virt, level);
            let entry_ptr = A::table_entry(table, index);
            let entry = unsafe { ptr::read_volatile(entry_ptr) };

            if !A::is_present(entry) {
                let next = allocator.alloc(1)?;
                unsafe { zero_page(next) };
                unsafe {
                    ptr::write_volatile(entry_ptr, A::make_table(next, flags));
                }
                table = next;
                continue;
            }

            if A::is_leaf(entry, level) {
                return Err(MappingError::AlreadyMapped);
            }

            table = A::entry_frame(entry);
        }

        let index = A::index_of(virt, leaf_level);
        let entry_ptr = A::table_entry(table, index);
        let entry = unsafe { ptr::read_volatile(entry_ptr) };
        if A::is_present(entry) {
            return Err(MappingError::AlreadyMapped);
        }

        unsafe {
            ptr::write_volatile(entry_ptr, A::make_leaf(frame, leaf_level, flags));
        }
        A::invalidate(virt);
        Ok(())
    }

    pub fn map_alloc<F: FrameAllocator>(
        &mut self,
        virt: VirtAddr,
        size: MapSize,
        flags: MapFlags,
        allocator: &mut F,
    ) -> Result<(), MappingError> {
        let frame = allocator
            .alloc(frame_count_for_size(size))
            .map_err(|_| MappingError::OutOfMemory)?;
        self.map(virt, frame, size, flags, allocator)
    }

    pub fn unmap<F: FrameAllocator>(
        &mut self,
        virt: VirtAddr,
        allocator: &mut F,
    ) -> Result<UnmapResult, MappingError> {
        self.unmap_inner(virt, true, allocator)
    }

    pub fn unmap_preserve<F: FrameAllocator>(
        &mut self,
        virt: VirtAddr,
        allocator: &mut F,
    ) -> Result<UnmapResult, MappingError> {
        self.unmap_inner(virt, false, allocator)
    }

    fn unmap_inner<F: FrameAllocator>(
        &mut self,
        virt: VirtAddr,
        release_frame: bool,
        allocator: &mut F,
    ) -> Result<UnmapResult, MappingError> {
        let mut table = self.root;
        let mut walk = [None; MAX_PAGE_TABLE_LEVELS];

        for (level, slot) in walk.iter_mut().enumerate().take(A::LEVELS) {
            let index = A::index_of(virt, level);
            let entry_ptr = A::table_entry(table, index);
            let entry = unsafe { ptr::read_volatile(entry_ptr) };
            if !A::is_present(entry) {
                return Err(MappingError::NotMapped);
            }

            *slot = Some(WalkEntry { table, index });
            if A::is_leaf(entry, level) {
                let size = map_size_for_level::<A>(level)?;
                unsafe {
                    ptr::write_volatile(entry_ptr, 0);
                }
                A::invalidate(virt);

                let frame = A::entry_frame(entry);
                let remaining_refs = if release_frame {
                    allocator.release(frame, frame_count_for_size(size))?
                } else {
                    1
                };
                self.prune_empty_tables(&walk, level, allocator)?;

                return Ok(UnmapResult {
                    frame,
                    remaining_refs,
                    size,
                });
            }

            table = A::entry_frame(entry);
        }

        Err(MappingError::NotMapped)
    }

    pub fn protect(&mut self, virt: VirtAddr, flags: MapFlags) -> Result<(), MappingError> {
        let mut table = self.root;

        for level in 0..A::LEVELS {
            let index = A::index_of(virt, level);
            let entry_ptr = A::table_entry(table, index);
            let entry = unsafe { ptr::read_volatile(entry_ptr) };
            if !A::is_present(entry) {
                return Err(MappingError::NotMapped);
            }

            if A::is_leaf(entry, level) {
                let frame = A::entry_frame(entry);
                unsafe {
                    ptr::write_volatile(entry_ptr, A::make_leaf(frame, level, flags));
                }
                A::invalidate(virt);
                return Ok(());
            }

            table = A::entry_frame(entry);
        }

        Err(MappingError::NotMapped)
    }

    fn prune_empty_tables<F: FrameAllocator>(
        &self,
        walk: &[Option<WalkEntry>; MAX_PAGE_TABLE_LEVELS],
        leaf_level: usize,
        allocator: &mut F,
    ) -> Result<(), MappingError> {
        let mut level = leaf_level;
        while level > 0 {
            let child = walk[level].ok_or(MappingError::NotMapped)?;
            if !table_is_empty::<A>(child.table) {
                break;
            }

            let parent = walk[level - 1].ok_or(MappingError::NotMapped)?;
            let parent_entry = A::table_entry(parent.table, parent.index);
            unsafe {
                ptr::write_volatile(parent_entry, 0);
            }
            allocator.release(child.table, 1)?;
            level -= 1;
        }

        Ok(())
    }
}

fn table_is_empty<A: PageTableArch>(table: PhysFrame) -> bool {
    for index in 0..A::ENTRY_COUNT {
        let entry = unsafe { ptr::read_volatile(A::table_entry(table, index)) };
        if A::is_present(entry) {
            return false;
        }
    }
    true
}

unsafe fn zero_page(frame: PhysFrame) {
    let page = phys_to_virt(frame.start_address().as_u64()) as *mut u8;
    unsafe {
        ptr::write_bytes(page, 0, PAGE_SIZE as usize);
    }
}

fn map_size_for_level<A: PageTableArch>(level: usize) -> Result<MapSize, MappingError> {
    match A::page_size(level) {
        Some(4096) => Ok(MapSize::Size4KiB),
        Some(0x20_0000) => Ok(MapSize::Size2MiB),
        Some(0x4000_0000) => Ok(MapSize::Size1GiB),
        _ => Err(MappingError::UnsupportedSize),
    }
}

const fn frame_count_for_size(size: MapSize) -> usize {
    match size {
        MapSize::Size4KiB => 1,
        MapSize::Size2MiB => (0x20_0000 / PAGE_SIZE) as usize,
        MapSize::Size1GiB => (0x4000_0000 / PAGE_SIZE) as usize,
    }
}
