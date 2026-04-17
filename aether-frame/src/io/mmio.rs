use core::ptr::NonNull;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::libs::spin::SpinLock;
use crate::mm::{
    AddressSpace, ArchitecturePageTable, MapFlags, MapSize, PAGE_SIZE, PhysAddr, PhysFrame,
    VirtAddr, frame_allocator,
};

const MMIO_REMAP_BASE: u64 = 0xffff_c000_0000_0000;

static NEXT_MMIO_VADDR: AtomicU64 = AtomicU64::new(MMIO_REMAP_BASE);
static MMIO_REMAP_LOCK: SpinLock<()> = SpinLock::new(());

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemapError {
    InvalidSize,
    AddressSpaceExhausted,
    Map(crate::mm::MappingError),
}

impl From<crate::mm::MappingError> for RemapError {
    fn from(value: crate::mm::MappingError) -> Self {
        Self::Map(value)
    }
}

pub struct Mmio<T> {
    ptr: NonNull<T>,
}

impl<T> Copy for Mmio<T> {}
impl<T> Clone for Mmio<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Mmio<T> {
    /// Creates a volatile MMIO accessor from a mapped address.
    ///
    /// # Safety
    /// The caller must ensure `ptr` points at a valid MMIO register of type
    /// `T`, is properly aligned, and remains mapped for the lifetime of this
    /// accessor.
    pub const unsafe fn new(ptr: *mut T) -> Self {
        Self {
            ptr: unsafe { NonNull::new_unchecked(ptr) },
        }
    }

    #[must_use]
    pub fn read(self) -> T
    where
        T: Copy,
    {
        unsafe { self.ptr.as_ptr().read_volatile() }
    }

    pub fn write(self, value: T) {
        unsafe {
            self.ptr.as_ptr().write_volatile(value);
        }
    }
}

#[derive(Clone, Copy)]
pub struct MmioRegion {
    base: NonNull<u8>,
    len: usize,
}

unsafe impl Send for MmioRegion {}
unsafe impl Sync for MmioRegion {}

impl MmioRegion {
    #[must_use]
    pub const fn base(&self) -> *mut u8 {
        self.base.as_ptr()
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[must_use]
    pub fn as_ptr<T>(&self, offset: usize) -> Option<*mut T> {
        if offset.checked_add(core::mem::size_of::<T>())? > self.len {
            return None;
        }

        Some(unsafe { self.base.as_ptr().add(offset).cast::<T>() })
    }

    /// Builds a typed MMIO register accessor inside this remapped region.
    ///
    /// # Safety
    /// The caller must ensure the selected offset and type `T` match the
    /// device's register layout.
    #[must_use]
    pub unsafe fn register<T>(&self, offset: usize) -> Option<Mmio<T>> {
        Some(unsafe { Mmio::new(self.as_ptr::<T>(offset)?) })
    }
}

pub fn remap_mmio(phys_base: u64, size: usize) -> Result<MmioRegion, RemapError> {
    if size == 0 {
        return Err(RemapError::InvalidSize);
    }

    let _guard = MMIO_REMAP_LOCK.lock_irqsave();
    let page_base = phys_base & !(PAGE_SIZE - 1);
    let page_offset = (phys_base - page_base) as usize;
    let total_len = align_up(page_offset + size, PAGE_SIZE as usize);
    let virt_base = allocate_mmio_range(total_len as u64)?;

    let mut address_space = AddressSpace::<ArchitecturePageTable>::current();
    let mut allocator = frame_allocator().lock_irqsave();

    for page_offset_bytes in (0..total_len).step_by(PAGE_SIZE as usize) {
        let phys =
            PhysFrame::from_start_address(PhysAddr::new(page_base + page_offset_bytes as u64));
        let virt = VirtAddr::new(virt_base + page_offset_bytes as u64);
        address_space.map(
            virt,
            phys,
            MapSize::Size4KiB,
            MapFlags::READ | MapFlags::WRITE | MapFlags::NO_CACHE | MapFlags::GLOBAL,
            &mut *allocator,
        )?;
    }

    let ptr = (virt_base + page_offset as u64) as *mut u8;
    Ok(MmioRegion {
        base: NonNull::new(ptr).ok_or(RemapError::AddressSpaceExhausted)?,
        len: size,
    })
}

fn allocate_mmio_range(len: u64) -> Result<u64, RemapError> {
    let mut current = NEXT_MMIO_VADDR.load(Ordering::Acquire);
    loop {
        let next = current
            .checked_add(len)
            .ok_or(RemapError::AddressSpaceExhausted)?;
        match NEXT_MMIO_VADDR.compare_exchange(current, next, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return Ok(current),
            Err(observed) => current = observed,
        }
    }
}

const fn align_up(value: usize, align: usize) -> usize {
    (value + align - 1) & !(align - 1)
}
