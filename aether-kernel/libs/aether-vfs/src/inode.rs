extern crate alloc;

use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::any::Any;

use aether_frame::libs::spin::SpinLock;

use crate::file::{FlockOperation, FlockState};
use crate::{
    DirectoryEntry, FileAdvice, FileOperations, FsError, FsResult, IoctlResponse, MmapRequest,
    MmapResponse, NodeKind, NodeMetadata, PollEvents, SharedWaitListener, SuperBlock,
    SuperBlockRef, WaitQueue,
};

pub type NodeRef = Arc<Inode>;

pub trait InodeOperations: Any + Send + Sync {
    fn as_any(&self) -> &dyn Any;

    fn name(&self) -> &str;

    fn kind(&self) -> NodeKind;

    fn lookup(&self, _name: &str) -> Option<NodeRef> {
        None
    }

    fn insert_child(&self, _name: String, _node: NodeRef) -> FsResult<()> {
        Err(FsError::NotDirectory)
    }

    fn remove_child(&self, _name: &str, _remove_directory: bool) -> FsResult<()> {
        Err(FsError::NotDirectory)
    }

    fn rename_child(
        &self,
        _old_name: &str,
        _new_parent: &NodeRef,
        _new_name: String,
        _replace: bool,
    ) -> FsResult<()> {
        Err(FsError::NotDirectory)
    }

    fn link_child(&self, _name: String, _existing: &NodeRef) -> FsResult<()> {
        Err(FsError::NotDirectory)
    }

    fn create_file(&self, _name: String, _mode: u32) -> FsResult<NodeRef> {
        Err(FsError::NotDirectory)
    }

    fn create_dir(&self, _name: String, _mode: u32) -> FsResult<NodeRef> {
        Err(FsError::NotDirectory)
    }

    fn create_symlink(&self, _name: String, _target: String, _mode: u32) -> FsResult<NodeRef> {
        Err(FsError::NotDirectory)
    }

    fn create_socket(&self, _name: String, _mode: u32) -> FsResult<NodeRef> {
        Err(FsError::NotDirectory)
    }

    fn entries(&self) -> Vec<DirectoryEntry> {
        Vec::new()
    }

    fn file_ops(&self) -> Option<&dyn FileOperations> {
        None
    }

    fn symlink_target(&self) -> Option<&str> {
        None
    }

    fn device_numbers(&self) -> Option<(u32, u32)> {
        None
    }

    fn mode(&self) -> Option<u32> {
        None
    }

    fn set_mode(&self, _mode: u32) -> FsResult<()> {
        Err(FsError::Unsupported)
    }

    fn set_owner(&self, _uid: u32, _gid: u32) -> FsResult<()> {
        Err(FsError::Unsupported)
    }

    fn metadata(&self) -> NodeMetadata {
        let (default_mode, size) = match self.kind() {
            NodeKind::Directory => (0o040755, 0),
            NodeKind::File => (
                0o100644,
                self.file_ops().map_or(0, FileOperations::size) as u64,
            ),
            NodeKind::Socket => (0o140777, 0),
            NodeKind::Symlink => (
                0o120777,
                self.symlink_target().map(str::len).unwrap_or(0) as u64,
            ),
            NodeKind::BlockDevice => (0o060644, 0),
            NodeKind::CharDevice => (0o020666, 0),
            NodeKind::Fifo => (0o010600, 0),
        };
        let mut metadata = NodeMetadata {
            mode: self.mode().unwrap_or(default_mode),
            size,
            blocks: size.div_ceil(512),
            ..NodeMetadata::default()
        };
        if let Some((major, minor)) = self.device_numbers() {
            metadata.rdev_major = major;
            metadata.rdev_minor = minor;
        }
        metadata
    }
}

pub struct Inode {
    superblock: SpinLock<Option<Weak<SuperBlock>>>,
    flock: SpinLock<FlockState>,
    flock_waiters: WaitQueue,
    operations: Arc<dyn InodeOperations>,
}

impl Inode {
    pub fn new(operations: Arc<dyn InodeOperations>) -> NodeRef {
        Arc::new(Self {
            superblock: SpinLock::new(None),
            flock: SpinLock::new(FlockState::default()),
            flock_waiters: WaitQueue::new(),
            operations,
        })
    }

    pub fn bind_superblock(&self, superblock: &SuperBlockRef) {
        *self.superblock.lock_irqsave() = Some(Arc::downgrade(superblock));
    }

    pub fn superblock(&self) -> Option<SuperBlockRef> {
        self.superblock
            .lock_irqsave()
            .as_ref()
            .and_then(Weak::upgrade)
    }

    pub fn operations(&self) -> &dyn InodeOperations {
        self.operations.as_ref()
    }

    pub fn name(&self) -> &str {
        self.operations.name()
    }

    pub fn kind(&self) -> NodeKind {
        self.operations.kind()
    }

    pub fn lookup(&self, name: &str) -> Option<NodeRef> {
        self.operations.lookup(name)
    }

    pub fn insert_child(&self, name: String, node: NodeRef) -> FsResult<()> {
        self.operations.insert_child(name, node)
    }

    pub fn remove_child(&self, name: &str, remove_directory: bool) -> FsResult<()> {
        self.operations.remove_child(name, remove_directory)
    }

    pub fn rename_child(
        &self,
        old_name: &str,
        new_parent: &NodeRef,
        new_name: String,
        replace: bool,
    ) -> FsResult<()> {
        self.operations
            .rename_child(old_name, new_parent, new_name, replace)
    }

    pub fn link_child(&self, name: String, existing: &NodeRef) -> FsResult<()> {
        self.operations.link_child(name, existing)
    }

    pub fn create_file(&self, name: String, mode: u32) -> FsResult<NodeRef> {
        self.operations.create_file(name, mode)
    }

    pub fn create_dir(&self, name: String, mode: u32) -> FsResult<NodeRef> {
        self.operations.create_dir(name, mode)
    }

    pub fn create_symlink(&self, name: String, target: String, mode: u32) -> FsResult<NodeRef> {
        self.operations.create_symlink(name, target, mode)
    }

    pub fn create_socket(&self, name: String, mode: u32) -> FsResult<NodeRef> {
        self.operations.create_socket(name, mode)
    }

    pub fn entries(&self) -> Vec<DirectoryEntry> {
        self.operations.entries()
    }

    pub fn open(&self) {
        if let Some(file) = self.file() {
            file.open();
        }
    }

    pub fn release(&self) {
        if let Some(file) = self.file() {
            file.release();
        }
    }

    pub fn read(&self, offset: usize, buffer: &mut [u8]) -> FsResult<usize> {
        let file = self.file().ok_or(FsError::NotFile)?;
        file.read(offset, buffer)
    }

    pub fn write(&self, offset: usize, buffer: &[u8]) -> FsResult<usize> {
        let file = self.file().ok_or(FsError::NotFile)?;
        file.write(offset, buffer)
    }

    pub fn size(&self) -> usize {
        self.file().map_or(0, FileOperations::size)
    }

    pub fn truncate(&self, size: usize) -> FsResult<()> {
        let file = self.file().ok_or(FsError::NotFile)?;
        file.truncate(size)
    }

    pub fn advise(&self, offset: u64, len: u64, advice: FileAdvice) -> FsResult<()> {
        let file = self.file().ok_or(FsError::NotFile)?;
        file.advise(offset, len, advice)
    }

    pub fn fallocate(&self, mode: u32, offset: u64, len: u64) -> FsResult<()> {
        let file = self.file().ok_or(FsError::NotFile)?;
        file.fallocate(mode, offset, len)
    }

    pub fn flock(&self, owner: u64, operation: FlockOperation) -> FsResult<()> {
        let mut state = self.flock.lock_irqsave();
        let changed = match operation {
            FlockOperation::Unlock => {
                let removed_shared = state.shared.remove(&owner);
                let removed_exclusive = state.exclusive == Some(owner);
                if removed_exclusive {
                    state.exclusive = None;
                }
                removed_shared || removed_exclusive
            }
            FlockOperation::Shared => {
                if let Some(holder) = state.exclusive
                    && holder != owner
                {
                    return Err(FsError::WouldBlock);
                }
                let was_exclusive = state.exclusive == Some(owner);
                if was_exclusive {
                    state.exclusive = None;
                }
                let inserted = state.shared.insert(owner);
                was_exclusive || inserted
            }
            FlockOperation::Exclusive => {
                if state.exclusive == Some(owner) {
                    false
                } else {
                    let shared_by_others = state
                        .shared
                        .iter()
                        .any(|&shared_owner| shared_owner != owner);
                    if state.exclusive.is_some() || shared_by_others {
                        return Err(FsError::WouldBlock);
                    }
                    let removed_shared = state.shared.remove(&owner);
                    state.exclusive = Some(owner);
                    removed_shared || state.exclusive == Some(owner)
                }
            }
        };
        drop(state);
        if changed {
            self.flock_waiters.notify(PollEvents::LOCK);
        }
        Ok(())
    }

    pub fn register_flock_waiter(&self, listener: SharedWaitListener) -> u64 {
        self.flock_waiters.register(PollEvents::LOCK, listener)
    }

    pub fn unregister_flock_waiter(&self, waiter_id: u64) -> bool {
        self.flock_waiters.unregister(waiter_id)
    }

    pub fn wait_token(&self) -> u64 {
        self.file().map_or(0, FileOperations::wait_token)
    }

    pub fn register_waiter(
        &self,
        events: PollEvents,
        listener: SharedWaitListener,
    ) -> FsResult<Option<u64>> {
        let file = self.file().ok_or(FsError::NotFile)?;
        file.register_waiter(events, listener)
    }

    pub fn unregister_waiter(&self, waiter_id: u64) -> FsResult<()> {
        let file = self.file().ok_or(FsError::NotFile)?;
        file.unregister_waiter(waiter_id)
    }

    pub fn ioctl(&self, command: u64, argument: u64) -> FsResult<IoctlResponse> {
        let file = self.file().ok_or(FsError::NotFile)?;
        file.ioctl(command, argument)
    }

    pub fn poll(&self, events: PollEvents) -> FsResult<PollEvents> {
        let file = self.file().ok_or(FsError::NotFile)?;
        file.poll(events)
    }

    pub fn mmap(&self, request: MmapRequest) -> FsResult<MmapResponse> {
        let file = self.file().ok_or(FsError::NotFile)?;
        file.mmap(request)
    }

    pub fn file(&self) -> Option<&dyn FileOperations> {
        self.operations.file_ops()
    }

    pub fn symlink_target(&self) -> Option<&str> {
        self.operations.symlink_target()
    }

    pub fn device_numbers(&self) -> Option<(u32, u32)> {
        self.operations.device_numbers()
    }

    pub fn mode(&self) -> Option<u32> {
        self.operations.mode()
    }

    pub fn set_mode(&self, mode: u32) -> FsResult<()> {
        self.operations.set_mode(mode)
    }

    pub fn set_owner(&self, uid: u32, gid: u32) -> FsResult<()> {
        self.operations.set_owner(uid, gid)
    }

    pub fn metadata(&self) -> NodeMetadata {
        let mut metadata = self.operations.metadata();
        if metadata.device_id == 0
            && let Some(superblock) = self.superblock()
        {
            metadata.device_id = superblock.device_id();
        }
        metadata
    }
}
