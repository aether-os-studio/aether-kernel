use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;

use aether_frame::libs::spin::SpinLock;

use crate::{
    FileOperations, FsError, FsResult, NodeRef, PollEvents, SharedWaitListener, WaitListener,
    WaitQueue,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EpollEvents {
    bits: u32,
}

impl EpollEvents {
    pub const IN: u32 = 0x001;
    pub const PRI: u32 = 0x002;
    pub const OUT: u32 = 0x004;
    pub const ERR: u32 = 0x008;
    pub const HUP: u32 = 0x010;
    pub const NVAL: u32 = 0x020;
    pub const RDNORM: u32 = 0x040;
    pub const RDBAND: u32 = 0x080;
    pub const WRNORM: u32 = 0x100;
    pub const WRBAND: u32 = 0x200;
    pub const MSG: u32 = 0x400;
    pub const RDHUP: u32 = 0x2000;
    pub const EXCLUSIVE: u32 = 1 << 28;
    pub const WAKEUP: u32 = 1 << 29;
    pub const ONESHOT: u32 = 1 << 30;
    pub const ET: u32 = 1 << 31;

    pub const fn empty() -> Self {
        Self { bits: 0 }
    }

    pub const fn from_bits(bits: u32) -> Self {
        Self { bits }
    }

    pub const fn bits(self) -> u32 {
        self.bits
    }

    pub const fn contains(self, flag: u32) -> bool {
        (self.bits & flag) != 0
    }

    pub const fn insert(&mut self, flag: u32) {
        self.bits |= flag;
    }

    pub fn to_poll_events(self) -> PollEvents {
        let mut events = PollEvents::empty();
        if self.contains(Self::IN) || self.contains(Self::RDNORM) || self.contains(Self::RDBAND) {
            events = events | PollEvents::READ;
        }
        if self.contains(Self::OUT) || self.contains(Self::WRNORM) || self.contains(Self::WRBAND) {
            events = events | PollEvents::WRITE;
        }
        if self.contains(Self::ERR) || self.contains(Self::HUP) || self.contains(Self::NVAL) {
            events = events | PollEvents::ERROR;
        }
        events
    }

    pub fn from_poll_events(events: PollEvents) -> Self {
        let mut bits = 0u32;
        if events.contains(PollEvents::READ) {
            bits |= Self::IN;
        }
        if events.contains(PollEvents::WRITE) {
            bits |= Self::OUT;
        }
        if events.contains(PollEvents::ERROR) {
            bits |= Self::ERR;
        }
        Self { bits }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct EpollData {
    pub u64_: u64,
}

impl EpollData {
    pub const fn from_fd(fd: u32) -> Self {
        Self { u64_: fd as u64 }
    }

    pub const fn from_u32(val: u32) -> Self {
        Self { u64_: val as u64 }
    }

    pub const fn from_u64(val: u64) -> Self {
        Self { u64_: val }
    }

    pub const fn as_fd(self) -> u32 {
        self.u64_ as u32
    }

    pub const fn as_u32(self) -> u32 {
        self.u64_ as u32
    }

    pub const fn as_u64(self) -> u64 {
        self.u64_
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct EpollEvent {
    pub events: EpollEvents,
    pub data: EpollData,
}

impl EpollEvent {
    pub const fn new(events: EpollEvents, data: EpollData) -> Self {
        Self { events, data }
    }

    pub fn from_raw(raw: u64) -> Self {
        Self {
            events: EpollEvents::from_bits((raw & 0xFFFF_FFFF) as u32),
            data: EpollData::from_u64(raw >> 32),
        }
    }

    pub fn to_raw(self) -> u64 {
        (self.events.bits() as u64) | (self.data.u64_ << 32)
    }

    pub fn to_bytes(self) -> [u8; 12] {
        let events_bytes = self.events.bits().to_ne_bytes();
        let data_bytes = self.data.u64_.to_ne_bytes();
        let mut result = [0u8; 12];
        result[..4].copy_from_slice(&events_bytes);
        result[4..].copy_from_slice(&data_bytes);
        result
    }

    pub fn from_bytes(bytes: &[u8; 12]) -> Self {
        let events = u32::from_ne_bytes(bytes[..4].try_into().unwrap_or([0; 4]));
        let data = u64::from_ne_bytes(bytes[4..].try_into().unwrap_or([0; 8]));
        Self {
            events: EpollEvents::from_bits(events),
            data: EpollData::from_u64(data),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EpollCtlOp {
    Add = 1,
    Mod = 2,
    Del = 3,
}

impl EpollCtlOp {
    pub const fn from_raw(raw: i32) -> Option<Self> {
        match raw {
            1 => Some(Self::Add),
            2 => Some(Self::Mod),
            3 => Some(Self::Del),
            _ => None,
        }
    }
}

struct EpollNotifier {
    ready: SpinLock<BTreeMap<u64, EpollEvents>>,
    waiters: WaitQueue,
}

impl EpollNotifier {
    fn mark_ready(&self, fd: u64, events: EpollEvents) {
        if events.bits() == 0 {
            return;
        }

        let mut ready = self.ready.lock_irqsave();
        ready
            .entry(fd)
            .and_modify(|stored| stored.insert(events.bits()))
            .or_insert(events);
        drop(ready);
        self.waiters.notify(PollEvents::READ);
    }

    fn take_ready(&self, max_events: usize) -> Vec<(u64, EpollEvents)> {
        let mut ready = self.ready.lock_irqsave();
        let fds = ready.keys().copied().take(max_events).collect::<Vec<_>>();
        let mut events = Vec::with_capacity(fds.len());
        for fd in fds {
            if let Some(bits) = ready.remove(&fd) {
                events.push((fd, bits));
            }
        }
        events
    }

    fn has_pending(&self) -> bool {
        !self.ready.lock_irqsave().is_empty()
    }
}

struct EpollTargetListener {
    fd: u64,
    notifier: Arc<EpollNotifier>,
}

impl WaitListener for EpollTargetListener {
    fn wake(&self, events: PollEvents) {
        self.notifier
            .mark_ready(self.fd, EpollEvents::from_poll_events(events));
    }
}

struct EpollInterest {
    node: NodeRef,
    event: EpollEvent,
    waiter_id: Option<u64>,
}

pub struct EpollInstance {
    notifier: Arc<EpollNotifier>,
    interests: SpinLock<BTreeMap<u64, EpollInterest>>,
}

impl EpollInstance {
    pub fn new() -> Self {
        Self {
            notifier: Arc::new(EpollNotifier {
                ready: SpinLock::new(BTreeMap::new()),
                waiters: WaitQueue::new(),
            }),
            interests: SpinLock::new(BTreeMap::new()),
        }
    }

    fn register_interest_waiter(
        &self,
        fd: u64,
        node: &NodeRef,
        event: EpollEvent,
    ) -> FsResult<Option<u64>> {
        let poll_events = event.events.to_poll_events();
        if poll_events.bits() == 0 {
            return Ok(None);
        }

        let file = node.file().ok_or(FsError::NotFile)?;
        let listener: SharedWaitListener = Arc::new(EpollTargetListener {
            fd,
            notifier: self.notifier.clone(),
        });
        let waiter_id = file.register_waiter(poll_events, listener)?;

        if let Ok(ready) = node.poll(poll_events) {
            let ready = EpollEvents::from_poll_events(ready);
            if ready.bits() != 0 {
                self.notifier.mark_ready(fd, ready);
            }
        }

        Ok(waiter_id)
    }

    fn unregister_interest_waiter(interest: &EpollInterest) {
        if let Some(waiter_id) = interest.waiter_id
            && let Some(file) = interest.node.file()
        {
            let _ = file.unregister_waiter(waiter_id);
        }
    }

    pub fn ctl(&self, op: EpollCtlOp, fd: u64, node: NodeRef, event: EpollEvent) -> FsResult<()> {
        let mut interests = self.interests.lock_irqsave();

        match op {
            EpollCtlOp::Add => {
                if interests.contains_key(&fd) {
                    return Err(FsError::AlreadyExists);
                }
                let waiter_id = self.register_interest_waiter(fd, &node, event)?;
                interests.insert(
                    fd,
                    EpollInterest {
                        node,
                        event,
                        waiter_id,
                    },
                );
            }
            EpollCtlOp::Mod => {
                let old = interests.remove(&fd).ok_or(FsError::NotFound)?;
                Self::unregister_interest_waiter(&old);
                let waiter_id = self.register_interest_waiter(fd, &node, event)?;
                interests.insert(
                    fd,
                    EpollInterest {
                        node,
                        event,
                        waiter_id,
                    },
                );
            }
            EpollCtlOp::Del => {
                let old = interests.remove(&fd).ok_or(FsError::NotFound)?;
                Self::unregister_interest_waiter(&old);
                let _ = self.notifier.ready.lock_irqsave().remove(&fd);
            }
        }

        Ok(())
    }

    pub fn wait(&self, max_events: usize) -> FsResult<Vec<EpollEvent>> {
        if max_events == 0 {
            return Err(FsError::InvalidInput);
        }

        let ready = self.notifier.take_ready(max_events);
        let interests = self.interests.lock_irqsave();
        let mut result = Vec::with_capacity(ready.len());

        for (fd, ready_bits) in ready {
            let Some(interest) = interests.get(&fd) else {
                continue;
            };

            let mut final_events = ready_bits;
            if interest.event.events.contains(EpollEvents::ONESHOT) {
                final_events.insert(EpollEvents::ONESHOT);
            }
            result.push(EpollEvent::new(final_events, interest.event.data));

            if !interest.event.events.contains(EpollEvents::ET)
                && !interest.event.events.contains(EpollEvents::ONESHOT)
            {
                let poll_events = interest.event.events.to_poll_events();
                if let Ok(still_ready) = interest.node.poll(poll_events) {
                    let still_ready = EpollEvents::from_poll_events(still_ready);
                    if still_ready.bits() != 0 {
                        self.notifier.mark_ready(fd, still_ready);
                    }
                }
            }
        }

        Ok(result)
    }

    pub fn has_pending_events(&self) -> bool {
        self.notifier.has_pending()
    }

    pub fn interest_count(&self) -> usize {
        self.interests.lock_irqsave().len()
    }
}

impl Default for EpollInstance {
    fn default() -> Self {
        Self::new()
    }
}

impl FileOperations for EpollInstance {
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn poll(&self, events: PollEvents) -> FsResult<PollEvents> {
        let mut result = PollEvents::empty();
        if events.contains(PollEvents::READ) && self.has_pending_events() {
            result = result | PollEvents::READ;
        }
        Ok(result)
    }

    fn size(&self) -> usize {
        self.interest_count()
    }

    fn register_waiter(
        &self,
        events: PollEvents,
        listener: SharedWaitListener,
    ) -> FsResult<Option<u64>> {
        Ok(Some(self.notifier.waiters.register(events, listener)))
    }

    fn unregister_waiter(&self, waiter_id: u64) -> FsResult<()> {
        let _ = self.notifier.waiters.unregister(waiter_id);
        Ok(())
    }
}

pub type SharedEpollInstance = Arc<EpollInstance>;

pub fn create_epoll_instance() -> SharedEpollInstance {
    Arc::new(EpollInstance::new())
}
