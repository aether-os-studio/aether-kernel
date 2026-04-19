mod address;
mod buddy;
mod frame;
mod mapper;

use core::alloc::{GlobalAlloc, Layout};
use core::mem::MaybeUninit;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

use good_memory_allocator::Allocator;

use crate::boot;
use crate::libs::spin::{LocalIrqDisabled, PreemptDisabled, SpinLock, SpinLockGuard};

pub use self::address::{PAGE_SHIFT, PAGE_SIZE, PhysAddr, VirtAddr};
pub use self::buddy::BuddyAllocator;
pub use self::frame::{FrameAllocError, FrameAllocator, PhysFrame};
pub use self::mapper::{AddressSpace, MapFlags, MapSize, MappingError, PageTableArch, UnmapResult};
#[cfg(target_arch = "x86_64")]
pub use crate::arch::mm::new_user_root;
pub use crate::arch::mm::{ArchitecturePageTable, PageTableEntry};

pub struct FrameAllocatorSlot {
    ready: AtomicBool,
    value: SpinLock<MaybeUninit<BuddyAllocator>>,
}

pub struct FrameAllocatorGuard<'a> {
    guard: SpinLockGuard<'a, MaybeUninit<BuddyAllocator>, PreemptDisabled>,
}

unsafe impl Sync for FrameAllocatorSlot {}

impl FrameAllocatorSlot {
    const fn uninit() -> Self {
        Self {
            ready: AtomicBool::new(false),
            value: SpinLock::new(MaybeUninit::uninit()),
        }
    }

    unsafe fn init_with<E>(
        &self,
        init: impl FnOnce(*mut BuddyAllocator) -> Result<(), E>,
    ) -> Result<(), E> {
        let mut slot = self.value.lock();
        init(slot.as_mut_ptr())?;
        self.ready.store(true, Ordering::Release);
        Ok(())
    }

    pub fn lock(&self) -> FrameAllocatorGuard<'_> {
        assert!(
            self.ready.load(Ordering::Acquire),
            "frame allocator must be initialized before use"
        );
        FrameAllocatorGuard {
            guard: self.value.lock(),
        }
    }
}

impl Deref for FrameAllocatorGuard<'_> {
    type Target = BuddyAllocator;

    fn deref(&self) -> &Self::Target {
        unsafe { self.guard.assume_init_ref() }
    }
}

impl DerefMut for FrameAllocatorGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { self.guard.assume_init_mut() }
    }
}

static FRAME_ALLOCATOR: FrameAllocatorSlot = FrameAllocatorSlot::uninit();

pub struct LockedHeap(SpinLock<Allocator, LocalIrqDisabled>);

impl LockedHeap {
    pub const fn empty() -> Self {
        Self(SpinLock::new(Allocator::empty()))
    }

    pub unsafe fn init(&self, start: usize, size: usize) {
        self.0.lock().init(start, size);
    }
}
unsafe impl GlobalAlloc for LockedHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.0.lock().alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        self.0.lock().dealloc(ptr);
    }
}

#[global_allocator]
static HEAP_ALLOCATOR: LockedHeap = LockedHeap::empty();

const KERNEL_HEAP_START: usize = 0xffff_e000_0000_0000;
const KERNEL_HEAP_SIZE: usize = 128 * 1024 * 1024;

pub fn init() -> Result<(), FrameAllocError> {
    let mut current_address_space = AddressSpace::<ArchitecturePageTable>::current();
    unsafe {
        FRAME_ALLOCATOR.init_with(|slot| {
            BuddyAllocator::bootstrap_in_place(slot, &boot::info().memory_map, 16)
        })?;
    }
    for addr in
        (KERNEL_HEAP_START..KERNEL_HEAP_START + KERNEL_HEAP_SIZE).step_by(PAGE_SIZE as usize)
    {
        current_address_space
            .map_alloc(
                VirtAddr::new(addr as u64),
                MapSize::Size4KiB,
                MapFlags::READ | MapFlags::WRITE,
                &mut *frame_allocator().lock(),
            )
            .expect("Failed to map heap");
    }
    unsafe {
        HEAP_ALLOCATOR.init(KERNEL_HEAP_START, KERNEL_HEAP_SIZE);
    }
    Ok(())
}

pub fn frame_allocator() -> &'static FrameAllocatorSlot {
    &FRAME_ALLOCATOR
}
