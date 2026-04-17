extern crate alloc;

use aether_frame::libs::spin::SpinLock;
use aether_vfs::{
    FileNode, FileOperations, FsError, FsResult, NodeRef, PollEvents, SharedWaitListener, WaitQueue,
};
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicU64, Ordering};

const PIPE_CAPACITY: usize = 64 * 1024;

pub fn anonymous_pipe() -> (NodeRef, NodeRef) {
    let state = Arc::new(PipeState::default());

    let read_end: NodeRef = FileNode::new_fifo(
        "pipe-read",
        Arc::new(PipeEndpoint::new(state.clone(), PipeEndpointKind::Read)),
    );
    let write_end: NodeRef = FileNode::new_fifo(
        "pipe-write",
        Arc::new(PipeEndpoint::new(state, PipeEndpointKind::Write)),
    );
    (read_end, write_end)
}

struct PipeState {
    inner: SpinLock<PipeBuffer>,
    version: AtomicU64,
    waiters: WaitQueue,
}

impl PipeState {
    fn version(&self) -> u64 {
        self.version.load(Ordering::Acquire)
    }

    fn bump(&self) {
        let _ = self.version.fetch_add(1, Ordering::AcqRel);
    }

    fn add_reader(&self) {
        let mut inner = self.inner.lock_irqsave();
        inner.readers = inner.readers.saturating_add(1);
        drop(inner);
        self.bump();
        self.waiters
            .notify(PollEvents::READ | PollEvents::WRITE | PollEvents::ERROR);
    }

    fn add_writer(&self) {
        let mut inner = self.inner.lock_irqsave();
        inner.writers = inner.writers.saturating_add(1);
        drop(inner);
        self.bump();
        self.waiters
            .notify(PollEvents::READ | PollEvents::WRITE | PollEvents::ERROR);
    }

    fn remove_reader(&self) {
        let mut inner = self.inner.lock_irqsave();
        inner.readers = inner.readers.saturating_sub(1);
        drop(inner);
        self.bump();
        self.waiters
            .notify(PollEvents::READ | PollEvents::WRITE | PollEvents::ERROR);
    }

    fn remove_writer(&self) {
        let mut inner = self.inner.lock_irqsave();
        inner.writers = inner.writers.saturating_sub(1);
        drop(inner);
        self.bump();
        self.waiters
            .notify(PollEvents::READ | PollEvents::WRITE | PollEvents::ERROR);
    }
}

impl Default for PipeState {
    fn default() -> Self {
        Self {
            inner: SpinLock::new(PipeBuffer::default()),
            version: AtomicU64::new(1),
            waiters: WaitQueue::new(),
        }
    }
}

#[derive(Default)]
struct PipeBuffer {
    bytes: VecDeque<u8>,
    readers: usize,
    writers: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PipeEndpointKind {
    Read,
    Write,
}

struct PipeEndpoint {
    state: Arc<PipeState>,
    kind: PipeEndpointKind,
}

impl PipeEndpoint {
    const fn new(state: Arc<PipeState>, kind: PipeEndpointKind) -> Self {
        Self { state, kind }
    }
}

impl FileOperations for PipeEndpoint {
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn open(&self) {
        match self.kind {
            PipeEndpointKind::Read => self.state.add_reader(),
            PipeEndpointKind::Write => self.state.add_writer(),
        }
    }

    fn release(&self) {
        match self.kind {
            PipeEndpointKind::Read => self.state.remove_reader(),
            PipeEndpointKind::Write => self.state.remove_writer(),
        }
    }

    fn read(&self, _offset: usize, buffer: &mut [u8]) -> FsResult<usize> {
        if self.kind != PipeEndpointKind::Read {
            return Err(FsError::InvalidInput);
        }

        let mut inner = self.state.inner.lock_irqsave();
        if !inner.bytes.is_empty() {
            let count = buffer.len().min(inner.bytes.len());
            for slot in &mut buffer[..count] {
                *slot = inner.bytes.pop_front().expect("pipe data available");
            }
            drop(inner);
            self.state.bump();
            self.state.waiters.notify(PollEvents::WRITE);
            return Ok(count);
        }
        if inner.writers == 0 {
            return Ok(0);
        }
        return Err(FsError::WouldBlock);
    }

    fn write(&self, _offset: usize, buffer: &[u8]) -> FsResult<usize> {
        if self.kind != PipeEndpointKind::Write {
            return Err(FsError::InvalidInput);
        }

        let mut written = 0usize;
        while written < buffer.len() {
            let mut inner = self.state.inner.lock_irqsave();
            if inner.readers == 0 {
                return if written == 0 {
                    Err(FsError::BrokenPipe)
                } else {
                    Ok(written)
                };
            }

            let space = PIPE_CAPACITY.saturating_sub(inner.bytes.len());
            if space == 0 {
                return Err(FsError::WouldBlock);
            }

            let chunk = (buffer.len() - written).min(space);
            inner.bytes.extend(&buffer[written..written + chunk]);
            written += chunk;
            drop(inner);
            self.state.bump();
            self.state.waiters.notify(PollEvents::READ);
        }
        Ok(written)
    }

    fn poll(&self, events: PollEvents) -> FsResult<PollEvents> {
        let inner = self.state.inner.lock_irqsave();
        let mut ready = PollEvents::empty();

        if self.kind == PipeEndpointKind::Read && events.contains(PollEvents::READ) {
            if !inner.bytes.is_empty() || inner.writers == 0 {
                ready = ready | PollEvents::READ;
            }
        }

        if self.kind == PipeEndpointKind::Write && events.contains(PollEvents::WRITE) {
            if inner.readers == 0 || inner.bytes.len() < PIPE_CAPACITY {
                ready = ready | PollEvents::WRITE;
            }
        }

        if self.kind == PipeEndpointKind::Write && inner.readers == 0 {
            ready = ready | PollEvents::ERROR;
        }

        Ok(ready)
    }

    fn wait_token(&self) -> u64 {
        self.state.version()
    }

    fn register_waiter(
        &self,
        events: PollEvents,
        listener: SharedWaitListener,
    ) -> FsResult<Option<u64>> {
        Ok(Some(self.state.waiters.register(events, listener)))
    }

    fn unregister_waiter(&self, waiter_id: u64) -> FsResult<()> {
        let _ = self.state.waiters.unregister(waiter_id);
        Ok(())
    }
}
