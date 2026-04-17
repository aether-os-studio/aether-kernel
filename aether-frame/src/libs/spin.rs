use core::cell::UnsafeCell;
use core::hint::spin_loop;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

use crate::preempt;

pub struct SpinLock<T> {
    locked: AtomicBool,
    value: UnsafeCell<T>,
}

unsafe impl<T: Send> Send for SpinLock<T> {}
unsafe impl<T: Send> Sync for SpinLock<T> {}

impl<T> SpinLock<T> {
    pub const fn new(value: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            value: UnsafeCell::new(value),
        }
    }

    /// Returns a mutable reference to the protected value without taking the lock.
    ///
    /// # Safety
    ///
    /// The caller must ensure there is no concurrent access to this lock and that
    /// no `SpinLockGuard` or other reference derived from this lock is alive while
    /// the returned reference is used.
    pub unsafe fn get_mut(&self) -> &mut T {
        unsafe { &mut *self.value.get() }
    }

    pub fn lock(&self) -> SpinLockGuard<'_, T> {
        preempt::disable();
        self.acquire_lock();

        SpinLockGuard { lock: self }
    }

    fn acquire_lock(&self) {
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            while self.locked.load(Ordering::Relaxed) {
                spin_loop();
            }
        }
    }
}

impl<T: Default> Default for SpinLock<T> {
    fn default() -> Self {
        Self {
            locked: AtomicBool::new(false),
            value: UnsafeCell::new(T::default()),
        }
    }
}

pub struct SpinLockGuard<'a, T> {
    lock: &'a SpinLock<T>,
}

impl<T> Deref for SpinLockGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.lock.value.get() }
    }
}

impl<T> DerefMut for SpinLockGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.lock.value.get() }
    }
}

impl<T> Drop for SpinLockGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
        preempt::enable();
    }
}
