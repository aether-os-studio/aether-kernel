use core::marker::PhantomData;

pub trait PortValue: Copy {
    unsafe fn read(port: u16) -> Self;
    unsafe fn write(port: u16, value: Self);
}

impl PortValue for u8 {
    unsafe fn read(port: u16) -> Self {
        unsafe { crate::arch::io::inb(port) }
    }

    unsafe fn write(port: u16, value: Self) {
        unsafe { crate::arch::io::outb(port, value) }
    }
}

impl PortValue for u16 {
    unsafe fn read(port: u16) -> Self {
        unsafe { crate::arch::io::inw(port) }
    }

    unsafe fn write(port: u16, value: Self) {
        unsafe { crate::arch::io::outw(port, value) }
    }
}

impl PortValue for u32 {
    unsafe fn read(port: u16) -> Self {
        unsafe { crate::arch::io::inl(port) }
    }

    unsafe fn write(port: u16, value: Self) {
        unsafe { crate::arch::io::outl(port, value) }
    }
}

pub struct Port<T> {
    port: u16,
    _marker: PhantomData<T>,
}

impl<T: PortValue> Port<T> {
    /// Creates a typed port-I/O accessor.
    ///
    /// # Safety
    /// The caller must ensure the selected I/O port belongs to the target
    /// device and that accesses of type `T` are valid for it.
    #[must_use]
    pub const unsafe fn new(port: u16) -> Self {
        Self {
            port,
            _marker: PhantomData,
        }
    }

    #[must_use]
    pub fn read(&self) -> T {
        unsafe { T::read(self.port) }
    }

    pub fn write(&self, value: T) {
        unsafe { T::write(self.port, value) }
    }
}
