extern crate alloc;

use alloc::sync::Arc;
use core::any::Any;
use core::sync::atomic::{AtomicU64, Ordering};

use aether_frame::libs::spin::SpinLock;
use aether_vfs::{FileOperations, FsError, FsResult, PollEvents, SharedWaitListener, WaitQueue};

pub const EFD_SEMAPHORE: u64 = 0x1;
pub const EFD_NONBLOCK: u64 = 0o0004000;
pub const EFD_CLOEXEC: u64 = 0o2000000;
pub const EFD_VALID_FLAGS: u64 = EFD_SEMAPHORE | EFD_NONBLOCK | EFD_CLOEXEC;

const EVENTFD_COUNTER_MAX: u64 = u64::MAX - 1;

pub fn create_eventfd(initval: u32, flags: u64) -> Arc<EventFdFile> {
    Arc::new(EventFdFile {
        inner: SpinLock::new(EventFdState {
            counter: initval as u64,
        }),
        semaphore: (flags & EFD_SEMAPHORE) != 0,
        version: AtomicU64::new(1),
        waiters: WaitQueue::new(),
    })
}

struct EventFdState {
    counter: u64,
}

pub struct EventFdFile {
    inner: SpinLock<EventFdState>,
    semaphore: bool,
    version: AtomicU64,
    waiters: WaitQueue,
}

impl EventFdFile {
    fn version(&self) -> u64 {
        self.version.load(Ordering::Acquire)
    }

    fn bump(&self) {
        let _ = self.version.fetch_add(1, Ordering::AcqRel);
    }
}

impl FileOperations for EventFdFile {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn read(&self, _offset: usize, buffer: &mut [u8]) -> FsResult<usize> {
        if buffer.len() < core::mem::size_of::<u64>() {
            return Err(FsError::InvalidInput);
        }

        let (value, wake_writers) = {
            let mut inner = self.inner.lock_irqsave();
            if inner.counter == 0 {
                return Err(FsError::WouldBlock);
            }

            let was_full = inner.counter == EVENTFD_COUNTER_MAX;
            let value = if self.semaphore {
                inner.counter -= 1;
                1
            } else {
                let value = inner.counter;
                inner.counter = 0;
                value
            };
            (value, was_full)
        };

        buffer[..8].copy_from_slice(&value.to_ne_bytes());
        self.bump();
        if wake_writers {
            self.waiters.notify(PollEvents::WRITE);
        }
        Ok(8)
    }

    fn write(&self, _offset: usize, buffer: &[u8]) -> FsResult<usize> {
        if buffer.len() < core::mem::size_of::<u64>() {
            return Err(FsError::InvalidInput);
        }

        let value = u64::from_ne_bytes(buffer[..8].try_into().map_err(|_| FsError::InvalidInput)?);
        if value == u64::MAX {
            return Err(FsError::InvalidInput);
        }

        let wake_readers = {
            let mut inner = self.inner.lock_irqsave();
            let available = EVENTFD_COUNTER_MAX.saturating_sub(inner.counter);
            if value > available {
                return Err(FsError::WouldBlock);
            }

            let was_empty = inner.counter == 0;
            inner.counter += value;
            was_empty && value != 0
        };

        if value != 0 {
            self.bump();
            if wake_readers {
                self.waiters.notify(PollEvents::READ);
            }
        }
        Ok(8)
    }

    fn poll(&self, events: PollEvents) -> FsResult<PollEvents> {
        let counter = self.inner.lock_irqsave().counter;
        let mut ready = PollEvents::empty();

        if events.contains(PollEvents::READ) && counter != 0 {
            ready = ready | PollEvents::READ;
        }
        if events.contains(PollEvents::WRITE) && counter < EVENTFD_COUNTER_MAX {
            ready = ready | PollEvents::WRITE;
        }

        Ok(ready)
    }

    fn wait_token(&self) -> u64 {
        self.version()
    }

    fn register_waiter(
        &self,
        events: PollEvents,
        listener: SharedWaitListener,
    ) -> FsResult<Option<u64>> {
        Ok(Some(self.waiters.register(events, listener)))
    }

    fn unregister_waiter(&self, waiter_id: u64) -> FsResult<()> {
        let _ = self.waiters.unregister(waiter_id);
        Ok(())
    }
}
