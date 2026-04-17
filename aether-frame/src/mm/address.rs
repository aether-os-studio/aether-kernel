#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct PhysAddr(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct VirtAddr(u64);

pub const PAGE_SHIFT: usize = 12;
pub const PAGE_SIZE: u64 = 1 << PAGE_SHIFT;

impl PhysAddr {
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn as_u64(self) -> u64 {
        self.0
    }

    #[must_use]
    pub const fn is_aligned(self, align: u64) -> bool {
        self.0 & (align - 1) == 0
    }

    #[must_use]
    pub const fn align_down(self, align: u64) -> Self {
        Self(self.0 & !(align - 1))
    }
}

impl VirtAddr {
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn as_u64(self) -> u64 {
        self.0
    }

    #[must_use]
    pub const fn is_aligned(self, align: u64) -> bool {
        self.0 & (align - 1) == 0
    }
}
