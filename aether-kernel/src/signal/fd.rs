extern crate alloc;

use alloc::collections::VecDeque;
use alloc::sync::Arc;
use core::any::Any;
use core::sync::atomic::{AtomicU64, Ordering};

use aether_frame::libs::spin::SpinLock;
use aether_vfs::{FileOperations, FsError, FsResult, PollEvents, SharedWaitListener, WaitQueue};

use super::{SigSet, SignalInfo, SignalState};

const SIGNALFD_SIGINFO_SIZE: usize = 128;
const SIGNALFD_QUEUE_LIMIT: usize = 64;

pub fn create_signalfd(_state: SignalState, mask: SigSet) -> Arc<SignalFdFile> {
    Arc::new(SignalFdFile {
        mask: SpinLock::new(mask),
        queue: SpinLock::new(VecDeque::new()),
        version: AtomicU64::new(1),
        waiters: WaitQueue::new(),
    })
}

pub struct SignalFdFile {
    mask: SpinLock<SigSet>,
    queue: SpinLock<VecDeque<SignalInfo>>,
    version: AtomicU64,
    waiters: WaitQueue,
}

impl SignalFdFile {
    pub fn mask(&self) -> SigSet {
        *self.mask.lock()
    }

    pub fn set_mask(&self, mask: SigSet) {
        *self.mask.lock() = mask;
    }

    pub fn with_signal_state(&self, state: SignalState) -> Arc<Self> {
        create_signalfd(state, self.mask())
    }

    pub fn notify_signal(&self, info: SignalInfo) {
        let bit = super::sigbit(info.signal);
        if bit == 0 || (self.mask() & bit) == 0 {
            return;
        }

        {
            let mut queue = self.queue.lock();
            if queue.len() >= SIGNALFD_QUEUE_LIMIT {
                let _ = queue.pop_front();
            }
            queue.push_back(info);
        }

        let _ = self.version.fetch_add(1, Ordering::AcqRel);
        self.waiters.notify(PollEvents::READ);
    }

    fn version(&self) -> u64 {
        self.version.load(Ordering::Acquire)
    }
}

impl FileOperations for SignalFdFile {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn read(&self, _offset: usize, buffer: &mut [u8]) -> FsResult<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        if buffer.len() < SIGNALFD_SIGINFO_SIZE {
            return Err(FsError::InvalidInput);
        }

        let mut written = 0usize;
        while (buffer.len() - written) >= SIGNALFD_SIGINFO_SIZE {
            let Some(info) = self.queue.lock().pop_front() else {
                break;
            };
            let bytes = serialize_signalfd_siginfo(info);
            buffer[written..written + SIGNALFD_SIGINFO_SIZE].copy_from_slice(bytes.as_slice());
            written += SIGNALFD_SIGINFO_SIZE;
        }

        if written == 0 {
            return Err(FsError::WouldBlock);
        }
        let _ = self.version.fetch_add(1, Ordering::AcqRel);
        Ok(written)
    }

    fn poll(&self, events: PollEvents) -> FsResult<PollEvents> {
        let ready = if events.contains(PollEvents::READ) && !self.queue.lock().is_empty() {
            PollEvents::READ
        } else {
            PollEvents::empty()
        };
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
        if !events.contains(PollEvents::READ) {
            return Ok(None);
        }
        Ok(Some(self.waiters.register(PollEvents::READ, listener)))
    }

    fn unregister_waiter(&self, waiter_id: u64) -> FsResult<()> {
        let _ = self.waiters.unregister(waiter_id);
        Ok(())
    }
}

fn serialize_signalfd_siginfo(info: SignalInfo) -> [u8; SIGNALFD_SIGINFO_SIZE] {
    let mut bytes = [0u8; SIGNALFD_SIGINFO_SIZE];
    bytes[0..4].copy_from_slice(&(info.signal as u32).to_ne_bytes());
    bytes[4..8].copy_from_slice(&0i32.to_ne_bytes());
    bytes[8..12].copy_from_slice(&info.code.to_ne_bytes());
    bytes[12..16].copy_from_slice(&info.pid.to_ne_bytes());
    bytes[16..20].copy_from_slice(&info.uid.to_ne_bytes());
    bytes[40..44].copy_from_slice(&info.status.to_ne_bytes());
    bytes
}
