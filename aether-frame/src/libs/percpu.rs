use core::cell::UnsafeCell;
use core::marker::PhantomData;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

pub const MAX_PERCPU_CPUS: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PerCpuError {
    InvalidCpu,
    AlreadyInitialized,
    Uninitialized,
}

struct PerCpuSlot<T> {
    ready: AtomicBool,
    value: UnsafeCell<MaybeUninit<T>>,
    _marker: PhantomData<*const ()>,
}

unsafe impl<T: Send> Sync for PerCpuSlot<T> {}

impl<T> PerCpuSlot<T> {
    const fn uninit() -> Self {
        Self {
            ready: AtomicBool::new(false),
            value: UnsafeCell::new(MaybeUninit::uninit()),
            _marker: PhantomData,
        }
    }
}

pub struct PerCpu<T, const N: usize> {
    slots: [PerCpuSlot<T>; N],
}

unsafe impl<T: Send, const N: usize> Sync for PerCpu<T, N> {}

impl<T, const N: usize> PerCpu<T, N> {
    #[must_use]
    pub const fn uninit() -> Self {
        Self {
            slots: [const { PerCpuSlot::uninit() }; N],
        }
    }

    pub fn init(&self, cpu_index: usize, value: T) -> Result<(), PerCpuError> {
        let slot = self.slots.get(cpu_index).ok_or(PerCpuError::InvalidCpu)?;

        if slot.ready.load(Ordering::Acquire) {
            return Err(PerCpuError::AlreadyInitialized);
        }

        unsafe {
            (*slot.value.get()).write(value);
        }
        slot.ready.store(true, Ordering::Release);
        Ok(())
    }

    pub fn get(&self, cpu_index: usize) -> Result<&T, PerCpuError> {
        let slot = self.slots.get(cpu_index).ok_or(PerCpuError::InvalidCpu)?;
        if !slot.ready.load(Ordering::Acquire) {
            return Err(PerCpuError::Uninitialized);
        }

        Ok(unsafe { (*slot.value.get()).assume_init_ref() })
    }

    pub fn with<R>(&self, cpu_index: usize, f: impl FnOnce(&T) -> R) -> Result<R, PerCpuError> {
        Ok(f(self.get(cpu_index)?))
    }

    pub fn with_mut<R>(
        &self,
        cpu_index: usize,
        f: impl FnOnce(&mut T) -> R,
    ) -> Result<R, PerCpuError> {
        let slot = self.slots.get(cpu_index).ok_or(PerCpuError::InvalidCpu)?;
        if !slot.ready.load(Ordering::Acquire) {
            return Err(PerCpuError::Uninitialized);
        }

        Ok(f(unsafe { (*slot.value.get()).assume_init_mut() }))
    }
}

impl<T, const N: usize> Default for PerCpu<T, N> {
    fn default() -> Self {
        Self::uninit()
    }
}

pub struct PerCpuPtr<T, const N: usize> {
    slots: [AtomicUsize; N],
    _marker: PhantomData<*mut T>,
}

unsafe impl<T, const N: usize> Sync for PerCpuPtr<T, N> {}

impl<T, const N: usize> PerCpuPtr<T, N> {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            slots: [const { AtomicUsize::new(0) }; N],
            _marker: PhantomData,
        }
    }

    pub fn store(&self, cpu_index: usize, ptr: *mut T) -> Result<(), PerCpuError> {
        let slot = self.slots.get(cpu_index).ok_or(PerCpuError::InvalidCpu)?;
        slot.store(ptr as usize, Ordering::Release);
        Ok(())
    }

    pub fn load(&self, cpu_index: usize) -> Result<*mut T, PerCpuError> {
        let slot = self.slots.get(cpu_index).ok_or(PerCpuError::InvalidCpu)?;
        Ok(slot.load(Ordering::Acquire) as *mut T)
    }
}

impl<T, const N: usize> Default for PerCpuPtr<T, N> {
    fn default() -> Self {
        Self::new()
    }
}
