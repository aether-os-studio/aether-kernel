use super::address::{PAGE_SHIFT, PhysAddr};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameAllocError {
    InvalidCount,
    OutOfMemory,
    InvalidFrame,
    InvalidCpu,
    InvalidMemoryMap,
    RefCountOverflow,
    RefCountUnderflow,
    MetadataExhausted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PhysFrame {
    start: PhysAddr,
}

impl PhysFrame {
    #[must_use]
    pub const fn from_start_address(start: PhysAddr) -> Self {
        Self { start }
    }

    #[must_use]
    pub const fn start_address(self) -> PhysAddr {
        self.start
    }

    #[must_use]
    pub const fn index(self) -> usize {
        (self.start.as_u64() >> PAGE_SHIFT) as usize
    }
}

pub trait FrameAllocator {
    fn alloc(&mut self, count: usize) -> Result<PhysFrame, FrameAllocError>;
    fn retain(&mut self, frame: PhysFrame) -> Result<usize, FrameAllocError>;
    fn release(&mut self, frame: PhysFrame, count: usize) -> Result<usize, FrameAllocError>;
    fn ref_count(&self, frame: PhysFrame) -> Option<usize>;
}
