extern crate alloc;

use alloc::sync::Arc;
use core::any::Any;
use core::sync::atomic::{AtomicU64, Ordering};

use aether_frame::libs::spin::SpinLock;
use aether_vfs::{
    FileNode, FileOperations, FsError, FsResult, NodeRef, PollEvents, SharedWaitListener, WaitQueue,
};

use crate::process::Pid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PidFdState {
    Alive,
    Exited,
    Reaped,
}

pub struct PidFdHandle {
    pid: Pid,
    state: SpinLock<PidFdState>,
    version: AtomicU64,
    waiters: WaitQueue,
}

impl PidFdHandle {
    pub fn new(pid: Pid) -> Arc<Self> {
        Arc::new(Self {
            pid,
            state: SpinLock::new(PidFdState::Alive),
            version: AtomicU64::new(1),
            waiters: WaitQueue::new(),
        })
    }

    pub const fn pid(&self) -> Pid {
        self.pid
    }

    pub fn mark_exited(&self) {
        let mut state = self.state.lock();
        if *state != PidFdState::Alive {
            return;
        }
        *state = PidFdState::Exited;
        drop(state);
        self.bump();
        self.waiters.notify(PollEvents::READ);
    }

    pub fn mark_reaped(&self) {
        let mut state = self.state.lock();
        if *state == PidFdState::Reaped {
            return;
        }
        *state = PidFdState::Reaped;
        drop(state);
        self.bump();
        self.waiters.notify(PollEvents::READ | PollEvents::HUP);
    }

    fn ready_events(&self, requested: PollEvents) -> PollEvents {
        match *self.state.lock() {
            PidFdState::Alive => PollEvents::empty(),
            PidFdState::Exited => {
                let mut ready = PollEvents::empty();
                if requested.contains(PollEvents::READ) {
                    ready = ready | PollEvents::READ;
                }
                ready
            }
            PidFdState::Reaped => {
                let mut ready = PollEvents::empty();
                if requested.contains(PollEvents::READ) {
                    ready = ready | PollEvents::READ;
                }
                if requested.contains(PollEvents::HUP) {
                    ready = ready | PollEvents::HUP;
                }
                ready
            }
        }
    }

    fn wait_token(&self) -> u64 {
        self.version.load(Ordering::Acquire)
    }

    fn bump(&self) {
        let _ = self.version.fetch_add(1, Ordering::AcqRel);
    }
}

pub struct PidFdFile {
    handle: Arc<PidFdHandle>,
}

impl PidFdFile {
    pub fn new(handle: Arc<PidFdHandle>) -> Arc<Self> {
        Arc::new(Self { handle })
    }

    pub fn handle(&self) -> &Arc<PidFdHandle> {
        &self.handle
    }
}

pub fn create_pidfd_node(handle: Arc<PidFdHandle>) -> NodeRef {
    // TODO: model pidfds as anon_inodefs nodes once the VFS can express anon inodes.
    let node = FileNode::new("pidfd", PidFdFile::new(handle));
    let _ = node.set_mode(0o100600);
    node
}

impl FileOperations for PidFdFile {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn read(&self, _offset: usize, _buffer: &mut [u8]) -> FsResult<usize> {
        // Linux pidfds are pollable, but read(2) currently returns EINVAL.
        // TODO: add pidfd-specific syscalls such as pidfd_send_signal/pidfd_getfd separately.
        Err(FsError::InvalidInput)
    }

    fn write(&self, _offset: usize, _buffer: &[u8]) -> FsResult<usize> {
        Err(FsError::InvalidInput)
    }

    fn poll(&self, events: PollEvents) -> FsResult<PollEvents> {
        Ok(self.handle.ready_events(events))
    }

    fn wait_token(&self) -> u64 {
        self.handle.wait_token()
    }

    fn register_waiter(
        &self,
        events: PollEvents,
        listener: SharedWaitListener,
    ) -> FsResult<Option<u64>> {
        if !events.intersects(PollEvents::READ | PollEvents::HUP) {
            return Ok(None);
        }
        Ok(Some(self.handle.waiters.register(events, listener)))
    }

    fn unregister_waiter(&self, waiter_id: u64) -> FsResult<()> {
        let _ = self.handle.waiters.unregister(waiter_id);
        Ok(())
    }
}
