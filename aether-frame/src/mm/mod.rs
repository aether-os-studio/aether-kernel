mod address;
mod buddy;
mod frame;
mod mapper;

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::marker::PhantomData;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicBool, Ordering};

use good_memory_allocator::Allocator;

use crate::boot;
use crate::libs::spin::{LocalIrqDisabled, SpinLock};

pub use self::address::{PAGE_SHIFT, PAGE_SIZE, PhysAddr, VirtAddr};
pub use self::buddy::{BuddyAllocatorError, BuddyFrameAllocator};
pub use self::frame::{FrameAllocError, FrameAllocator, PhysFrame};
pub use self::mapper::{AddressSpace, MapFlags, MapSize, MappingError, PageTableArch, UnmapResult};
#[cfg(target_arch = "x86_64")]
pub use crate::arch::mm::new_user_root;
pub use crate::arch::mm::{ArchitecturePageTable, PageTableEntry};

struct GlobalSlot<T> {
    ready: AtomicBool,
    value: UnsafeCell<MaybeUninit<T>>,
    _marker: PhantomData<*const ()>,
}

unsafe impl<T> Sync for GlobalSlot<T> {}

impl<T> GlobalSlot<T> {
    const fn uninit() -> Self {
        Self {
            ready: AtomicBool::new(false),
            value: UnsafeCell::new(MaybeUninit::uninit()),
            _marker: PhantomData,
        }
    }

    unsafe fn write(&self, value: T) {
        unsafe {
            (*self.value.get()).write(value);
        }
        self.ready.store(true, Ordering::Release);
    }

    fn get(&self) -> Option<&T> {
        self.ready
            .load(Ordering::Acquire)
            .then(|| unsafe { (*self.value.get()).assume_init_ref() })
    }
}

static FRAME_ALLOCATOR: GlobalSlot<SpinLock<BuddyFrameAllocator>> = GlobalSlot::uninit();

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

pub fn init() -> Result<(), BuddyAllocatorError> {
    let allocator = unsafe { BuddyFrameAllocator::bootstrap(&boot::info().memory_map)? };
    let mut current_address_space = AddressSpace::<ArchitecturePageTable>::current();
    unsafe {
        FRAME_ALLOCATOR.write(SpinLock::new(allocator));
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

pub fn frame_allocator() -> &'static SpinLock<BuddyFrameAllocator> {
    FRAME_ALLOCATOR
        .get()
        .expect("frame allocator must be initialized before use")
}
