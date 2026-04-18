extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicU64, Ordering};

use aether_frame::libs::spin::{LocalIrqDisabled, SpinLock};

use crate::PollEvents;

pub trait WaitListener: Send + Sync {
    fn wake(&self, events: PollEvents);
}

pub type SharedWaitListener = Arc<dyn WaitListener>;

struct WaitRegistration {
    events: PollEvents,
    listener: SharedWaitListener,
}

pub struct WaitQueue {
    next_id: AtomicU64,
    registrations: SpinLock<BTreeMap<u64, WaitRegistration>, LocalIrqDisabled>,
}

impl WaitQueue {
    pub const fn new() -> Self {
        Self {
            next_id: AtomicU64::new(1),
            registrations: SpinLock::new(BTreeMap::new()),
        }
    }

    pub fn register(&self, events: PollEvents, listener: SharedWaitListener) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::AcqRel);
        self.registrations
            .lock()
            .insert(id, WaitRegistration { events, listener });
        id
    }

    pub fn unregister(&self, id: u64) -> bool {
        self.registrations.lock().remove(&id).is_some()
    }

    pub fn notify(&self, events: PollEvents) {
        let listeners = self
            .registrations
            .lock()
            .values()
            .filter_map(|registration| {
                let matched = events.intersection(registration.events);
                (matched != PollEvents::empty()).then(|| (registration.listener.clone(), matched))
            })
            .collect::<alloc::vec::Vec<_>>();

        for (listener, matched) in listeners {
            listener.wake(matched);
        }
    }
}

impl Default for WaitQueue {
    fn default() -> Self {
        Self::new()
    }
}
