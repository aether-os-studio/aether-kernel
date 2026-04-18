use core::cell::UnsafeCell;
use core::hint::spin_loop;
use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

pub trait SpinGuardian {
    type Guard;

    fn guard() -> Self::Guard;
}

pub enum PreemptDisabled {}

impl SpinGuardian for PreemptDisabled {
    type Guard = crate::preempt::DisabledPreemptGuard;

    fn guard() -> Self::Guard {
        crate::preempt::disable_preempt()
    }
}

pub enum LocalIrqDisabled {}

pub struct DisabledInterruptGuard {
    state: crate::arch::interrupt::InterruptState,
}

impl DisabledInterruptGuard {
    fn new() -> Self {
        Self {
            state: crate::interrupt::disable(),
        }
    }
}

impl Drop for DisabledInterruptGuard {
    fn drop(&mut self) {
        crate::interrupt::restore(self.state);
    }
}

impl SpinGuardian for LocalIrqDisabled {
    type Guard = DisabledInterruptGuard;

    fn guard() -> Self::Guard {
        DisabledInterruptGuard::new()
    }
}

#[repr(transparent)]
pub struct SpinLock<T, G = PreemptDisabled> {
    _guard: PhantomData<G>,
    inner: SpinLockInner<T>,
}

struct SpinLockInner<T> {
    locked: AtomicBool,
    value: UnsafeCell<T>,
}

unsafe impl<T: Send, G> Send for SpinLock<T, G> {}
unsafe impl<T: Send, G> Sync for SpinLock<T, G> {}

impl<T, G> SpinLock<T, G> {
    pub const fn new(value: T) -> Self {
        Self {
            _guard: PhantomData,
            inner: SpinLockInner {
                locked: AtomicBool::new(false),
                value: UnsafeCell::new(value),
            },
        }
    }
}

impl<T> SpinLock<T, PreemptDisabled> {
    #[must_use]
    pub fn disable_irq(&self) -> &SpinLock<T, LocalIrqDisabled> {
        let ptr =
            self as *const SpinLock<T, PreemptDisabled> as *const SpinLock<T, LocalIrqDisabled>;
        unsafe { &*ptr }
    }
}

impl<T, G: SpinGuardian> SpinLock<T, G> {
    /// Returns a mutable reference to the protected value without taking the lock.
    ///
    /// # Safety
    ///
    /// The caller must ensure there is no concurrent access to this lock and that
    /// no `SpinLockGuard` or other reference derived from this lock is alive while
    /// the returned reference is used.
    pub fn get_mut(&mut self) -> &mut T {
        self.inner.value.get_mut()
    }

    pub fn lock(&self) -> SpinLockGuard<'_, T, G> {
        let guard = G::guard();
        self.acquire_lock();

        SpinLockGuard { lock: self, guard }
    }

    pub fn try_lock(&self) -> Option<SpinLockGuard<'_, T, G>> {
        let guard = G::guard();
        if self.try_acquire_lock() {
            return Some(SpinLockGuard { lock: self, guard });
        }
        None
    }

    fn acquire_lock(&self) {
        while !self.try_acquire_lock() {
            spin_loop();
        }
    }

    fn try_acquire_lock(&self) -> bool {
        self.inner
            .locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }
}

impl<T: Default, G> Default for SpinLock<T, G> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

pub struct SpinLockGuard<'a, T, G: SpinGuardian> {
    lock: &'a SpinLock<T, G>,
    guard: G::Guard,
}

impl<T, G: SpinGuardian> Deref for SpinLockGuard<'_, T, G> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        let _ = &self.guard;
        unsafe { &*self.lock.inner.value.get() }
    }
}

impl<T, G: SpinGuardian> DerefMut for SpinLockGuard<'_, T, G> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        let _ = &self.guard;
        unsafe { &mut *self.lock.inner.value.get() }
    }
}

impl<T, G: SpinGuardian> Drop for SpinLockGuard<'_, T, G> {
    fn drop(&mut self) {
        self.lock.inner.locked.store(false, Ordering::Release);
    }
}
