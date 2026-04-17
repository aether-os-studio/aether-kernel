extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;
use core::cmp::min;
use core::ptr;
use core::sync::atomic::{AtomicU32, Ordering as AtomicOrdering};
use core::sync::atomic::{AtomicU64, Ordering};

use aether_frame::boot::phys_to_virt;
use aether_frame::libs::spin::SpinLock;
use aether_frame::mm::{FrameAllocator, PAGE_SIZE, PhysFrame, frame_allocator};

use crate::{
    FileAdvice, Inode, InodeOperations, IoctlResponse, MmapCachePolicy, MmapRequest, MmapResponse,
    NodeRef, PollEvents, SharedWaitListener,
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

    fn fallocate(&self, _mode: u32, _offset: u64, _len: u64) -> FsResult<()> {
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
        let mut entries = self.entries.lock();
        if entries.contains_key(&name) {
            return Err(FsError::AlreadyExists);
        }
        entries.insert(name, node);
        Ok(())
    }

    fn remove(&self, name: &str, remove_directory: bool) -> FsResult<()> {
        let mut entries = self.entries.lock();
        let Some(node) = entries.get(name) else {
            return Err(FsError::NotFound);
        };
        let is_directory = node.kind() == NodeKind::Directory;
        if remove_directory != is_directory {
            return Err(FsError::InvalidInput);
        }
        let removed = entries.remove(name).expect("entry existed");
        if !remove_directory {
            adjust_link_count(&removed, -1);
        }
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
            let mut entries = self.entries.lock();
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
            let mut source_entries = self.entries.lock();
            let mut target_entries = target.entries.lock();
            rename_between_maps(
                &mut source_entries,
                &mut target_entries,
                old_name,
                new_name,
                replace,
            )
        } else {
            let mut target_entries = target.entries.lock();
            let mut source_entries = self.entries.lock();
            rename_between_maps(
                &mut source_entries,
                &mut target_entries,
                old_name,
                new_name,
                replace,
            )
        }
    }

    fn link_existing(&self, name: String, node: &NodeRef) -> FsResult<()> {
        if node.kind() == NodeKind::Directory {
            return Err(FsError::Unsupported);
        }
        self.insert(name, node.clone())?;
        adjust_link_count(node, 1);
        Ok(())
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
        self.entries.lock().get(name).cloned()
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

    fn link_child(&self, name: String, existing: &NodeRef) -> FsResult<()> {
        self.link_existing(name, existing)
    }

    fn create_file(&self, name: String, mode: u32) -> FsResult<NodeRef> {
        let node =
            FileNode::new_with_mode(name.clone(), mode, 0, Arc::new(MutableMemoryFile::new(&[])));
        self.insert(name, node.clone())?;
        Ok(node)
    }

    fn create_dir(&self, name: String, mode: u32) -> FsResult<NodeRef> {
        let device_id = self.metadata.lock().device_id;
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
            .lock()
            .iter()
            .map(|(name, node)| DirectoryEntry {
                name: name.clone(),
                kind: node.kind(),
            })
            .collect()
    }

    fn mode(&self) -> Option<u32> {
        Some(self.metadata.lock().mode)
    }

    fn set_mode(&self, mode: u32) -> FsResult<()> {
        self.metadata.lock().mode = mode;
        Ok(())
    }

    fn set_owner(&self, uid: u32, gid: u32) -> FsResult<()> {
        let mut metadata = self.metadata.lock();
        metadata.uid = uid;
        metadata.gid = gid;
        Ok(())
    }

    fn metadata(&self) -> NodeMetadata {
        *self.metadata.lock()
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
        let metadata = self.metadata.lock();
        ((self.kind == NodeKind::BlockDevice) || (self.kind == NodeKind::CharDevice))
            .then_some((metadata.rdev_major, metadata.rdev_minor))
    }

    fn mode(&self) -> Option<u32> {
        Some(self.metadata.lock().mode)
    }

    fn set_mode(&self, mode: u32) -> FsResult<()> {
        self.metadata.lock().mode = mode;
        Ok(())
    }

    fn set_owner(&self, uid: u32, gid: u32) -> FsResult<()> {
        let mut metadata = self.metadata.lock();
        metadata.uid = uid;
        metadata.gid = gid;
        Ok(())
    }

    fn metadata(&self) -> NodeMetadata {
        self.metadata
            .lock()
            .with_size(self.operations.size() as u64)
    }
}

pub struct MemoryFile {
    bytes: Arc<[u8]>,
}

pub struct MutableMemoryFile {
    bytes: SpinLock<Vec<u8>>,
}

struct SharedMemoryState {
    size: usize,
    pages: Vec<PhysFrame>,
}

pub struct SharedMemoryFile {
    state: SpinLock<SharedMemoryState>,
    seals: AtomicU32,
    allow_sealing: bool,
}

impl MutableMemoryFile {
    pub fn new(bytes: &[u8]) -> Self {
        Self {
            bytes: SpinLock::new(bytes.to_vec()),
        }
    }
}

impl SharedMemoryFile {
    pub fn new() -> Self {
        Self::new_with_sealing(false)
    }

    pub fn new_with_sealing(allow_sealing: bool) -> Self {
        Self {
            state: SpinLock::new(SharedMemoryState {
                size: 0,
                pages: Vec::new(),
            }),
            seals: AtomicU32::new(if allow_sealing { 0 } else { F_SEAL_SEAL }),
            allow_sealing,
        }
    }

    pub fn seals(&self) -> u32 {
        self.seals.load(AtomicOrdering::Acquire)
    }

    pub fn add_seals(&self, seals: u32) -> FsResult<()> {
        if !self.allow_sealing {
            return Err(FsError::InvalidInput);
        }

        loop {
            let current = self.seals();
            if (current & F_SEAL_SEAL) != 0 {
                return Err(FsError::InvalidInput);
            }
            let next = current | seals;
            if self
                .seals
                .compare_exchange(
                    current,
                    next,
                    AtomicOrdering::AcqRel,
                    AtomicOrdering::Acquire,
                )
                .is_ok()
            {
                return Ok(());
            }
        }
    }

    fn ensure_capacity(state: &mut SharedMemoryState, len: usize) -> FsResult<()> {
        let required_pages = len.div_ceil(PAGE_SIZE as usize);
        if required_pages <= state.pages.len() {
            return Ok(());
        }

        let mut allocator = frame_allocator().lock();
        while state.pages.len() < required_pages {
            let frame = allocator.alloc(1).map_err(|_| FsError::InvalidInput)?;
            zero_frame(frame);
            state.pages.push(frame);
        }
        Ok(())
    }

    fn zero_range(state: &SharedMemoryState, start: usize, end: usize) {
        if start >= end {
            return;
        }

        let page_size = PAGE_SIZE as usize;
        let mut current = start;
        while current < end {
            let page_index = current / page_size;
            let page_offset = current % page_size;
            let chunk = min(end - current, page_size - page_offset);
            let ptr = frame_ptr(state.pages[page_index]).wrapping_add(page_offset);
            unsafe {
                ptr.write_bytes(0, chunk);
            }
            current += chunk;
        }
    }

    fn copy_out(state: &SharedMemoryState, offset: usize, buffer: &mut [u8]) -> usize {
        if offset >= state.size {
            return 0;
        }

        let end = min(offset.saturating_add(buffer.len()), state.size);
        let page_size = PAGE_SIZE as usize;
        let mut current = offset;
        let mut copied = 0usize;
        while current < end {
            let page_index = current / page_size;
            let page_offset = current % page_size;
            let chunk = min(end - current, page_size - page_offset);
            let src = frame_ptr(state.pages[page_index]).wrapping_add(page_offset);
            unsafe {
                ptr::copy_nonoverlapping(src, buffer[copied..copied + chunk].as_mut_ptr(), chunk);
            }
            copied += chunk;
            current += chunk;
        }
        copied
    }

    fn copy_in(state: &mut SharedMemoryState, offset: usize, buffer: &[u8]) {
        let page_size = PAGE_SIZE as usize;
        let mut current = offset;
        let mut copied = 0usize;
        while copied < buffer.len() {
            let page_index = current / page_size;
            let page_offset = current % page_size;
            let chunk = min(buffer.len() - copied, page_size - page_offset);
            let dst = frame_ptr(state.pages[page_index]).wrapping_add(page_offset);
            unsafe {
                ptr::copy_nonoverlapping(buffer[copied..copied + chunk].as_ptr(), dst, chunk);
            }
            copied += chunk;
            current += chunk;
        }
    }

    fn ensure_grow_allowed(&self, new_end: usize, current_size: usize) -> FsResult<()> {
        if new_end > current_size && (self.seals() & F_SEAL_GROW) != 0 {
            return Err(FsError::InvalidInput);
        }
        Ok(())
    }

    fn ensure_shrink_allowed(&self, new_size: usize, current_size: usize) -> FsResult<()> {
        if new_size < current_size && (self.seals() & F_SEAL_SHRINK) != 0 {
            return Err(FsError::InvalidInput);
        }
        Ok(())
    }

    fn ensure_write_allowed(&self) -> FsResult<()> {
        if (self.seals() & (F_SEAL_WRITE | F_SEAL_FUTURE_WRITE)) != 0 {
            return Err(FsError::InvalidInput);
        }
        Ok(())
    }

    fn shared_pages(&self, offset: usize, len: usize) -> FsResult<Arc<[u64]>> {
        let state = self.state.lock();
        let end = offset.checked_add(len).ok_or(FsError::InvalidInput)?;
        let page_size = PAGE_SIZE as usize;
        let first_page = offset / page_size;
        let page_count = len.div_ceil(page_size);
        if end > state.size.saturating_add(page_size.saturating_sub(1))
            || first_page.saturating_add(page_count) > state.pages.len()
        {
            return Err(FsError::InvalidInput);
        }

        let mut pages = Vec::with_capacity(page_count);
        for frame in &state.pages[first_page..first_page + page_count] {
            pages.push(frame.start_address().as_u64());
        }
        Ok(Arc::from(pages))
    }
}

impl FileOperations for MutableMemoryFile {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn read(&self, offset: usize, buffer: &mut [u8]) -> FsResult<usize> {
        let bytes = self.bytes.lock();
        if offset >= bytes.len() {
            return Ok(0);
        }

        let len = core::cmp::min(buffer.len(), bytes.len() - offset);
        buffer[..len].copy_from_slice(&bytes[offset..offset + len]);
        Ok(len)
    }

    fn write(&self, offset: usize, buffer: &[u8]) -> FsResult<usize> {
        let mut bytes = self.bytes.lock();
        let end = offset.saturating_add(buffer.len());
        if end > bytes.len() {
            bytes.resize(end, 0);
        }
        bytes[offset..end].copy_from_slice(buffer);
        Ok(buffer.len())
    }

    fn size(&self) -> usize {
        self.bytes.lock().len()
    }

    fn truncate(&self, size: usize) -> FsResult<()> {
        self.bytes.lock().resize(size, 0);
        Ok(())
    }
}

impl Drop for SharedMemoryFile {
    fn drop(&mut self) {
        let mut state = self.state.lock();
        let mut allocator = frame_allocator().lock();
        for frame in state.pages.drain(..) {
            let _ = allocator.release(frame, 1);
        }
    }
}

impl FileOperations for SharedMemoryFile {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn read(&self, offset: usize, buffer: &mut [u8]) -> FsResult<usize> {
        let state = self.state.lock();
        Ok(Self::copy_out(&state, offset, buffer))
    }

    fn write(&self, offset: usize, buffer: &[u8]) -> FsResult<usize> {
        self.ensure_write_allowed()?;

        let mut state = self.state.lock();
        let old_size = state.size;
        let end = offset
            .checked_add(buffer.len())
            .ok_or(FsError::InvalidInput)?;
        self.ensure_grow_allowed(end, old_size)?;
        Self::ensure_capacity(&mut state, end)?;
        if offset > old_size {
            Self::zero_range(&state, old_size, offset);
        }
        Self::copy_in(&mut state, offset, buffer);
        if end > old_size {
            state.size = end;
        }
        Ok(buffer.len())
    }

    fn size(&self) -> usize {
        self.state.lock().size
    }

    fn truncate(&self, size: usize) -> FsResult<()> {
        let mut state = self.state.lock();
        self.ensure_grow_allowed(size, state.size)?;
        self.ensure_shrink_allowed(size, state.size)?;
        if size > state.size {
            let old_size = state.size;
            Self::ensure_capacity(&mut state, size)?;
            Self::zero_range(&state, old_size, size);
        } else {
            Self::zero_range(&state, size, state.size);
        }
        state.size = size;
        Ok(())
    }

    fn fallocate(&self, mode: u32, offset: u64, len: u64) -> FsResult<()> {
        const FALLOC_FL_KEEP_SIZE: u32 = 0x01;

        if (mode & !FALLOC_FL_KEEP_SIZE) != 0 {
            return Err(FsError::Unsupported);
        }

        let end = offset
            .checked_add(len)
            .and_then(|value| usize::try_from(value).ok())
            .ok_or(FsError::InvalidInput)?;
        let mut state = self.state.lock();
        self.ensure_grow_allowed(end, state.size)?;
        Self::ensure_capacity(&mut state, end)?;
        if (mode & FALLOC_FL_KEEP_SIZE) == 0 && end > state.size {
            let old_size = state.size;
            Self::zero_range(&state, old_size, end);
            state.size = end;
        }
        Ok(())
    }

    fn mmap(&self, request: MmapRequest) -> FsResult<MmapResponse> {
        if !request.offset.is_multiple_of(PAGE_SIZE) {
            return Err(FsError::InvalidInput);
        }
        let offset = usize::try_from(request.offset).map_err(|_| FsError::InvalidInput)?;
        let len = usize::try_from(request.length).map_err(|_| FsError::InvalidInput)?;
        if len == 0 {
            return Err(FsError::InvalidInput);
        }
        Ok(MmapResponse::shared_physical(
            self.shared_pages(offset, len)?,
            MmapCachePolicy::Cached,
        ))
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

const F_SEAL_SEAL: u32 = 0x0001;
const F_SEAL_SHRINK: u32 = 0x0002;
const F_SEAL_GROW: u32 = 0x0004;
const F_SEAL_WRITE: u32 = 0x0008;
const F_SEAL_FUTURE_WRITE: u32 = 0x0010;

fn frame_ptr(frame: PhysFrame) -> *mut u8 {
    phys_to_virt(frame.start_address().as_u64()) as *mut u8
}

fn zero_frame(frame: PhysFrame) {
    unsafe {
        frame_ptr(frame).write_bytes(0, PAGE_SIZE as usize);
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
        Some(self.metadata.lock().mode)
    }

    fn set_mode(&self, mode: u32) -> FsResult<()> {
        self.metadata.lock().mode = mode;
        Ok(())
    }

    fn set_owner(&self, uid: u32, gid: u32) -> FsResult<()> {
        let mut metadata = self.metadata.lock();
        metadata.uid = uid;
        metadata.gid = gid;
        Ok(())
    }

    fn metadata(&self) -> NodeMetadata {
        *self.metadata.lock()
    }
}

fn adjust_link_count(node: &NodeRef, delta: i32) {
    if let Some(file) = node.operations().as_any().downcast_ref::<FileNode>() {
        let mut metadata = file.metadata.lock();
        metadata.nlink = metadata.nlink.saturating_add_signed(delta);
    } else if let Some(symlink) = node.operations().as_any().downcast_ref::<SymlinkNode>() {
        let mut metadata = symlink.metadata.lock();
        metadata.nlink = metadata.nlink.saturating_add_signed(delta);
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
    if let Some(replaced) = target_entries.remove(&new_name)
        && !Arc::ptr_eq(&replaced, &node)
    {
        adjust_link_count(&replaced, -1);
    }
    target_entries.insert(new_name, node);
    Ok(())
}
