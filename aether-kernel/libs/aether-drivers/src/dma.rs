use core::ptr;

use aether_frame::boot::phys_to_virt;
use aether_frame::mm::{FrameAllocator, PAGE_SIZE, PhysFrame, frame_allocator};

#[derive(Debug)]
pub struct DmaRegion {
    frame: PhysFrame,
    pages: usize,
    len: usize,
    ptr: *mut u8,
}

unsafe impl Send for DmaRegion {}
unsafe impl Sync for DmaRegion {}

impl DmaRegion {
    pub fn new(len: usize) -> Result<Self, ()> {
        let pages = len.div_ceil(PAGE_SIZE as usize).max(1);
        let frame = frame_allocator()
            .lock_irqsave()
            .alloc(pages)
            .map_err(|_| ())?;
        let ptr = phys_to_virt(frame.start_address().as_u64()) as *mut u8;
        unsafe {
            ptr::write_bytes(ptr, 0, pages * PAGE_SIZE as usize);
        }
        Ok(Self {
            frame,
            pages,
            len: pages * PAGE_SIZE as usize,
            ptr,
        })
    }

    pub fn from_pages(pages: usize) -> Result<Self, ()> {
        Self::new((pages.max(1)) * PAGE_SIZE as usize)
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn phys_addr(&self) -> u64 {
        self.frame.start_address().as_u64()
    }

    pub fn zero(&mut self) {
        unsafe {
            ptr::write_bytes(self.ptr, 0, self.len);
        }
    }

    pub fn as_slice(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self.ptr, self.len) }
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        unsafe { core::slice::from_raw_parts_mut(self.ptr, self.len) }
    }

    pub fn as_ptr<T>(&self) -> *mut T {
        self.ptr.cast::<T>()
    }
}

impl Drop for DmaRegion {
    fn drop(&mut self) {
        let _ = frame_allocator()
            .lock_irqsave()
            .release(self.frame, self.pages);
    }
}
