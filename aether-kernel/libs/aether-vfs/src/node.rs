extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;
use core::sync::atomic::{AtomicU64, Ordering};

use aether_frame::libs::spin::SpinLock;

use crate::{
    FileAdvice, Inode, InodeOperations, IoctlResponse, MmapRequest, MmapResponse, NodeRef,
    PollEvents, SharedWaitListener,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsError {
    NotFound,
    NotDirectory,
    NotFile,
    AlreadyExists,
    Unsupported,
    InvalidInput,
    RootNotMounted,
    WouldBlock,
    BrokenPipe,
}

pub type FsResult<T> = Result<T, FsError>;

pub trait FileOperations: Any + Send + Sync {
    fn as_any(&self) -> &dyn Any;

    fn open(&self) {}

    fn release(&self) {}

    fn read(&self, _offset: usize, _buffer: &mut [u8]) -> FsResult<usize> {
        Err(FsError::Unsupported)
    }

    fn write(&self, _offset: usize, _buffer: &[u8]) -> FsResult<usize> {
        Err(FsError::Unsupported)
    }

    fn advise(&self, _offset: u64, _len: u64, _advice: FileAdvice) -> FsResult<()> {
        Ok(())
    }

    fn size(&self) -> usize {
        0
    }

    fn truncate(&self, _size: usize) -> FsResult<()> {
        Err(FsError::Unsupported)
    }

    fn wait_token(&self) -> u64 {
        0
    }

    fn register_waiter(
        &self,
        _events: PollEvents,
        _listener: SharedWaitListener,
    ) -> FsResult<Option<u64>> {
        Ok(None)
    }

    fn unregister_waiter(&self, _waiter_id: u64) -> FsResult<()> {
        Ok(())
    }

    fn ioctl(&self, _command: u64, _argument: u64) -> FsResult<IoctlResponse> {
        Err(FsError::Unsupported)
    }

    fn poll(&self, _events: PollEvents) -> FsResult<PollEvents> {
        Ok(PollEvents::empty())
    }

    fn mmap(&self, _request: MmapRequest) -> FsResult<MmapResponse> {
        Ok(MmapResponse::buffered())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    Directory,
    File,
    Socket,
    Symlink,
    BlockDevice,
    CharDevice,
    Fifo,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryEntry {
    pub name: String,
    pub kind: NodeKind,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NodeTimestamp {
    pub secs: i64,
    pub nanos: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeMetadata {
    pub device_id: u64,
    pub inode: u64,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub nlink: u32,
    pub rdev_major: u32,
    pub rdev_minor: u32,
    pub size: u64,
    pub block_size: u32,
    pub blocks: u64,
    pub atime: NodeTimestamp,
    pub mtime: NodeTimestamp,
    pub ctime: NodeTimestamp,
    pub btime: NodeTimestamp,
}

impl Default for NodeMetadata {
    fn default() -> Self {
        Self {
            device_id: 0,
            inode: next_inode(),
            mode: 0,
            uid: 0,
            gid: 0,
            nlink: 1,
            rdev_major: 0,
            rdev_minor: 0,
            size: 0,
            block_size: 4096,
            blocks: 0,
            atime: NodeTimestamp::default(),
            mtime: NodeTimestamp::default(),
            ctime: NodeTimestamp::default(),
            btime: NodeTimestamp::default(),
        }
    }
}

impl NodeMetadata {
    pub fn directory(mode: u32) -> Self {
        Self {
            mode,
            nlink: 2,
            ..Self::default()
        }
    }

    pub fn file(mode: u32) -> Self {
        Self {
            mode,
            ..Self::default()
        }
    }

    pub fn symlink(mode: u32, size: u64) -> Self {
        Self {
            mode,
            size,
            ..Self::default()
        }
    }

    pub fn device(mode: u32, major: u32, minor: u32) -> Self {
        Self {
            mode,
            rdev_major: major,
            rdev_minor: minor,
            ..Self::default()
        }
    }

    pub fn fifo(mode: u32) -> Self {
        Self {
            mode,
            ..Self::default()
        }
    }

    pub fn with_device_id(mut self, device_id: u64) -> Self {
        self.device_id = device_id;
        self
    }

    pub fn with_size(mut self, size: u64) -> Self {
        self.size = size;
        self.blocks = size.div_ceil(512);
        self
    }
}

pub struct DirectoryNode {
    name: String,
    metadata: SpinLock<NodeMetadata>,
    entries: SpinLock<BTreeMap<String, NodeRef>>,
}

impl DirectoryNode {
    pub fn new(name: impl Into<String>) -> NodeRef {
        Self::new_with_mode(name, 0o040755)
    }

    pub fn new_with_mode(name: impl Into<String>, mode: u32) -> NodeRef {
        Self::new_with_metadata(name, NodeMetadata::directory(mode))
    }

    pub fn new_with_metadata(name: impl Into<String>, metadata: NodeMetadata) -> NodeRef {
        Inode::new(Arc::new(Self {
            name: name.into(),
            metadata: SpinLock::new(metadata),
            entries: SpinLock::new(BTreeMap::new()),
        }))
    }

    fn insert(&self, name: impl Into<String>, node: NodeRef) -> FsResult<()> {
        let name = name.into();
        let mut entries = self.entries.lock_irqsave();
        if entries.contains_key(&name) {
            return Err(FsError::AlreadyExists);
        }
        entries.insert(name, node);
        Ok(())
    }

    fn remove(&self, name: &str, remove_directory: bool) -> FsResult<()> {
        let mut entries = self.entries.lock_irqsave();
        let Some(node) = entries.get(name) else {
            return Err(FsError::NotFound);
        };
        let is_directory = node.kind() == NodeKind::Directory;
        if remove_directory != is_directory {
            return Err(FsError::InvalidInput);
        }
        let _ = entries.remove(name);
        Ok(())
    }

    fn rename(
        &self,
        old_name: &str,
        new_parent: &NodeRef,
        new_name: String,
        replace: bool,
    ) -> FsResult<()> {
        let target = new_parent
            .operations()
            .as_any()
            .downcast_ref::<DirectoryNode>()
            .ok_or(FsError::NotDirectory)?;

        if core::ptr::eq(self, target) {
            let mut entries = self.entries.lock_irqsave();
            let node = entries.remove(old_name).ok_or(FsError::NotFound)?;
            if !replace && entries.contains_key(&new_name) {
                entries.insert(String::from(old_name), node);
                return Err(FsError::AlreadyExists);
            }
            let _ = entries.remove(&new_name);
            entries.insert(new_name, node);
            return Ok(());
        }

        let self_ptr = self as *const Self as usize;
        let target_ptr = target as *const DirectoryNode as usize;
        if self_ptr < target_ptr {
            let mut source_entries = self.entries.lock_irqsave();
            let mut target_entries = target.entries.lock_irqsave();
            rename_between_maps(
                &mut source_entries,
                &mut target_entries,
                old_name,
                new_name,
                replace,
            )
        } else {
            let mut target_entries = target.entries.lock_irqsave();
            let mut source_entries = self.entries.lock_irqsave();
            rename_between_maps(
                &mut source_entries,
                &mut target_entries,
                old_name,
                new_name,
                replace,
            )
        }
    }
}

impl InodeOperations for DirectoryNode {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> NodeKind {
        NodeKind::Directory
    }

    fn lookup(&self, name: &str) -> Option<NodeRef> {
        self.entries.lock_irqsave().get(name).cloned()
    }

    fn insert_child(&self, name: String, node: NodeRef) -> FsResult<()> {
        self.insert(name, node)
    }

    fn remove_child(&self, name: &str, remove_directory: bool) -> FsResult<()> {
        self.remove(name, remove_directory)
    }

    fn rename_child(
        &self,
        old_name: &str,
        new_parent: &NodeRef,
        new_name: String,
        replace: bool,
    ) -> FsResult<()> {
        self.rename(old_name, new_parent, new_name, replace)
    }

    fn create_file(&self, name: String, mode: u32) -> FsResult<NodeRef> {
        let node =
            FileNode::new_with_mode(name.clone(), mode, 0, Arc::new(MutableMemoryFile::new(&[])));
        self.insert(name, node.clone())?;
        Ok(node)
    }

    fn create_dir(&self, name: String, mode: u32) -> FsResult<NodeRef> {
        let device_id = self.metadata.lock_irqsave().device_id;
        let node = DirectoryNode::new_with_metadata(
            name.clone(),
            NodeMetadata::directory(mode).with_device_id(device_id),
        );
        self.insert(name, node.clone())?;
        Ok(node)
    }

    fn create_symlink(&self, name: String, target: String, mode: u32) -> FsResult<NodeRef> {
        let node = SymlinkNode::new_with_mode(name.clone(), target, mode);
        self.insert(name, node.clone())?;
        Ok(node)
    }

    fn create_socket(&self, name: String, mode: u32) -> FsResult<NodeRef> {
        let node = FileNode::new_socket(name.clone(), mode, Arc::new(MutableMemoryFile::new(&[])));
        self.insert(name, node.clone())?;
        Ok(node)
    }

    fn entries(&self) -> Vec<DirectoryEntry> {
        self.entries
            .lock_irqsave()
            .iter()
            .map(|(name, node)| DirectoryEntry {
                name: name.clone(),
                kind: node.kind(),
            })
            .collect()
    }

    fn mode(&self) -> Option<u32> {
        Some(self.metadata.lock_irqsave().mode)
    }

    fn set_mode(&self, mode: u32) -> FsResult<()> {
        self.metadata.lock_irqsave().mode = mode;
        Ok(())
    }

    fn set_owner(&self, uid: u32, gid: u32) -> FsResult<()> {
        let mut metadata = self.metadata.lock_irqsave();
        metadata.uid = uid;
        metadata.gid = gid;
        Ok(())
    }

    fn metadata(&self) -> NodeMetadata {
        *self.metadata.lock_irqsave()
    }
}

pub struct FileNode {
    name: String,
    kind: NodeKind,
    metadata: SpinLock<NodeMetadata>,
    operations: Arc<dyn FileOperations>,
}

impl FileNode {
    pub fn new(name: impl Into<String>, operations: Arc<dyn FileOperations>) -> NodeRef {
        Self::new_with_mode(name, 0o100644, 0, operations)
    }

    pub fn new_socket(
        name: impl Into<String>,
        mode: u32,
        operations: Arc<dyn FileOperations>,
    ) -> NodeRef {
        Inode::new(Arc::new(Self {
            name: name.into(),
            kind: NodeKind::Socket,
            metadata: SpinLock::new(NodeMetadata::file(mode)),
            operations,
        }))
    }

    pub fn new_with_mode(
        name: impl Into<String>,
        mode: u32,
        device_id: u64,
        operations: Arc<dyn FileOperations>,
    ) -> NodeRef {
        Inode::new(Arc::new(Self {
            name: name.into(),
            kind: NodeKind::File,
            metadata: SpinLock::new(NodeMetadata::file(mode).with_device_id(device_id)),
            operations,
        }))
    }

    pub fn new_block_device(
        name: impl Into<String>,
        major: u32,
        minor: u32,
        operations: Arc<dyn FileOperations>,
    ) -> NodeRef {
        Inode::new(Arc::new(Self {
            name: name.into(),
            kind: NodeKind::BlockDevice,
            metadata: SpinLock::new(NodeMetadata::device(0o060644, major, minor)),
            operations,
        }))
    }

    pub fn new_char_device(
        name: impl Into<String>,
        major: u32,
        minor: u32,
        operations: Arc<dyn FileOperations>,
    ) -> NodeRef {
        Inode::new(Arc::new(Self {
            name: name.into(),
            kind: NodeKind::CharDevice,
            metadata: SpinLock::new(NodeMetadata::device(0o020666, major, minor)),
            operations,
        }))
    }

    pub fn new_fifo(name: impl Into<String>, operations: Arc<dyn FileOperations>) -> NodeRef {
        Inode::new(Arc::new(Self {
            name: name.into(),
            kind: NodeKind::Fifo,
            metadata: SpinLock::new(NodeMetadata::fifo(0o010600)),
            operations,
        }))
    }
}

impl InodeOperations for FileNode {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> NodeKind {
        self.kind
    }

    fn file_ops(&self) -> Option<&dyn FileOperations> {
        Some(self.operations.as_ref())
    }

    fn device_numbers(&self) -> Option<(u32, u32)> {
        let metadata = self.metadata.lock_irqsave();
        ((self.kind == NodeKind::BlockDevice) || (self.kind == NodeKind::CharDevice))
            .then_some((metadata.rdev_major, metadata.rdev_minor))
    }

    fn mode(&self) -> Option<u32> {
        Some(self.metadata.lock_irqsave().mode)
    }

    fn set_mode(&self, mode: u32) -> FsResult<()> {
        self.metadata.lock_irqsave().mode = mode;
        Ok(())
    }

    fn set_owner(&self, uid: u32, gid: u32) -> FsResult<()> {
        let mut metadata = self.metadata.lock_irqsave();
        metadata.uid = uid;
        metadata.gid = gid;
        Ok(())
    }

    fn metadata(&self) -> NodeMetadata {
        self.metadata
            .lock_irqsave()
            .with_size(self.operations.size() as u64)
    }
}

pub struct MemoryFile {
    bytes: Arc<[u8]>,
}

pub struct MutableMemoryFile {
    bytes: SpinLock<Vec<u8>>,
}

impl MutableMemoryFile {
    pub fn new(bytes: &[u8]) -> Self {
        Self {
            bytes: SpinLock::new(bytes.to_vec()),
        }
    }
}

impl FileOperations for MutableMemoryFile {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn read(&self, offset: usize, buffer: &mut [u8]) -> FsResult<usize> {
        let bytes = self.bytes.lock_irqsave();
        if offset >= bytes.len() {
            return Ok(0);
        }

        let len = core::cmp::min(buffer.len(), bytes.len() - offset);
        buffer[..len].copy_from_slice(&bytes[offset..offset + len]);
        Ok(len)
    }

    fn write(&self, offset: usize, buffer: &[u8]) -> FsResult<usize> {
        let mut bytes = self.bytes.lock_irqsave();
        let end = offset.saturating_add(buffer.len());
        if end > bytes.len() {
            bytes.resize(end, 0);
        }
        bytes[offset..end].copy_from_slice(buffer);
        Ok(buffer.len())
    }

    fn size(&self) -> usize {
        self.bytes.lock_irqsave().len()
    }

    fn truncate(&self, size: usize) -> FsResult<()> {
        self.bytes.lock_irqsave().resize(size, 0);
        Ok(())
    }
}

impl MemoryFile {
    pub fn new(bytes: impl Into<Arc<[u8]>>) -> Self {
        Self {
            bytes: bytes.into(),
        }
    }
}

impl FileOperations for MemoryFile {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn read(&self, offset: usize, buffer: &mut [u8]) -> FsResult<usize> {
        if offset >= self.bytes.len() {
            return Ok(0);
        }

        let len = core::cmp::min(buffer.len(), self.bytes.len() - offset);
        buffer[..len].copy_from_slice(&self.bytes[offset..offset + len]);
        Ok(len)
    }

    fn size(&self) -> usize {
        self.bytes.len()
    }
}

pub struct SymlinkNode {
    name: String,
    target: String,
    metadata: SpinLock<NodeMetadata>,
}

impl SymlinkNode {
    pub fn new(name: impl Into<String>, target: impl Into<String>) -> NodeRef {
        Self::new_with_mode(name, target, 0o120777)
    }

    pub fn new_with_mode(name: impl Into<String>, target: impl Into<String>, mode: u32) -> NodeRef {
        let target = target.into();
        Inode::new(Arc::new(Self {
            name: name.into(),
            metadata: SpinLock::new(NodeMetadata::symlink(mode, target.len() as u64)),
            target,
        }))
    }
}

impl InodeOperations for SymlinkNode {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> NodeKind {
        NodeKind::Symlink
    }

    fn symlink_target(&self) -> Option<&str> {
        Some(&self.target)
    }

    fn mode(&self) -> Option<u32> {
        Some(self.metadata.lock_irqsave().mode)
    }

    fn set_mode(&self, mode: u32) -> FsResult<()> {
        self.metadata.lock_irqsave().mode = mode;
        Ok(())
    }

    fn set_owner(&self, uid: u32, gid: u32) -> FsResult<()> {
        let mut metadata = self.metadata.lock_irqsave();
        metadata.uid = uid;
        metadata.gid = gid;
        Ok(())
    }

    fn metadata(&self) -> NodeMetadata {
        *self.metadata.lock_irqsave()
    }
}

fn next_inode() -> u64 {
    static NEXT_INODE: AtomicU64 = AtomicU64::new(1);
    NEXT_INODE.fetch_add(1, Ordering::AcqRel)
}

fn rename_between_maps(
    source_entries: &mut BTreeMap<String, NodeRef>,
    target_entries: &mut BTreeMap<String, NodeRef>,
    old_name: &str,
    new_name: String,
    replace: bool,
) -> FsResult<()> {
    let node = source_entries.remove(old_name).ok_or(FsError::NotFound)?;
    if !replace && target_entries.contains_key(&new_name) {
        source_entries.insert(String::from(old_name), node);
        return Err(FsError::AlreadyExists);
    }
    let _ = target_entries.remove(&new_name);
    target_entries.insert(new_name, node);
    Ok(())
}
