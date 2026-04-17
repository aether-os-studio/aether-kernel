extern crate alloc;

use alloc::sync::Arc;
use core::any::Any;

use aether_frame::libs::spin::SpinLock;
use aether_vfs::{FileOperations, FsError, FsResult, PollEvents, SharedWaitListener};

use super::{SigSet, SignalInfo, SignalState};

const SIGNALFD_SIGINFO_SIZE: usize = 128;

pub fn create_signalfd(state: SignalState, mask: SigSet) -> Arc<SignalFdFile> {
    Arc::new(SignalFdFile {
        state,
        mask: SpinLock::new(mask),
    })
}

pub struct SignalFdFile {
    state: SignalState,
    mask: SpinLock<SigSet>,
}

impl SignalFdFile {
    pub fn mask(&self) -> SigSet {
        *self.mask.lock_irqsave()
    }

    pub fn set_mask(&self, mask: SigSet) {
        *self.mask.lock_irqsave() = mask;
        if self.state.has_pending_in_mask(mask) {
            self.state.notify_waiters();
        }
    }

    pub fn with_signal_state(&self, state: SignalState) -> Arc<Self> {
        create_signalfd(state, self.mask())
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

        let mask = self.mask();
        let mut written = 0usize;
        while (buffer.len() - written) >= SIGNALFD_SIGINFO_SIZE {
            let Some(info) = self.state.take_pending_in_mask(mask) else {
                break;
            };
            let bytes = serialize_signalfd_siginfo(info);
            buffer[written..written + SIGNALFD_SIGINFO_SIZE].copy_from_slice(bytes.as_slice());
            written += SIGNALFD_SIGINFO_SIZE;
        }

        if written == 0 {
            return Err(FsError::WouldBlock);
        }
        Ok(written)
    }

    fn poll(&self, events: PollEvents) -> FsResult<PollEvents> {
        let ready =
            if events.contains(PollEvents::READ) && self.state.has_pending_in_mask(self.mask()) {
                PollEvents::READ
            } else {
                PollEvents::empty()
            };
        Ok(ready)
    }

    fn register_waiter(
        &self,
        events: PollEvents,
        listener: SharedWaitListener,
    ) -> FsResult<Option<u64>> {
        if !events.contains(PollEvents::READ) {
            return Ok(None);
        }
        Ok(Some(self.state.register_waiter(listener)))
    }

    fn unregister_waiter(&self, waiter_id: u64) -> FsResult<()> {
        let _ = self.state.unregister_waiter(waiter_id);
        Ok(())
    }
}

fn serialize_signalfd_siginfo(info: SignalInfo) -> [u8; SIGNALFD_SIGINFO_SIZE] {
    let mut bytes = [0u8; SIGNALFD_SIGINFO_SIZE];
    bytes[0..4].copy_from_slice(&(info.signal as u32).to_ne_bytes());
    bytes[4..8].copy_from_slice(&0i32.to_ne_bytes());
    bytes[8..12].copy_from_slice(&info.code.to_ne_bytes());
    bytes[12..16].copy_from_slice(&0u32.to_ne_bytes());
    bytes[16..20].copy_from_slice(&0u32.to_ne_bytes());
    bytes[40..44].copy_from_slice(&info.status.to_ne_bytes());
    bytes
}
