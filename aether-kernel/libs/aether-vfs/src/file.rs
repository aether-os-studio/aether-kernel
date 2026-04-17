extern crate alloc;

use aether_frame::libs::spin::SpinLock;
use alloc::collections::BTreeSet;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::ops::BitOr;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::{Dentry, DentryRef, FsError, FsResult, NodeRef, SharedWaitListener};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileAdvice {
    Normal,
    Random,
    Sequential,
    WillNeed,
    DontNeed,
    NoReuse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OpenFlags {
    bits: u32,
}

impl OpenFlags {
    pub const READ: u32 = 1 << 0;
    pub const WRITE: u32 = 1 << 1;
    pub const APPEND: u32 = 1 << 2;
    pub const NONBLOCK: u32 = 1 << 3;
    pub const DIRECTORY: u32 = 1 << 4;

    pub const fn empty() -> Self {
        Self { bits: 0 }
    }

    pub const fn from_bits(bits: u32) -> Self {
        Self { bits }
    }

    pub const fn bits(self) -> u32 {
        self.bits
    }

    pub const fn contains(self, bit: u32) -> bool {
        (self.bits & bit) != 0
    }

    pub const fn can_read(self) -> bool {
        self.contains(Self::READ) || !self.contains(Self::WRITE)
    }

    pub const fn can_write(self) -> bool {
        self.contains(Self::WRITE)
    }

    pub const fn append(self) -> bool {
        self.contains(Self::APPEND)
    }

    pub const fn nonblock(self) -> bool {
        self.contains(Self::NONBLOCK)
    }

    pub const fn directory(self) -> bool {
        self.contains(Self::DIRECTORY)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IoctlResponse {
    None(u64),
    Data(Vec<u8>),
    DataValue(Vec<u8>, u64),
}

impl IoctlResponse {
    pub const fn success() -> Self {
        Self::None(0)
    }

    pub const fn from_value(value: u64) -> Self {
        Self::None(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PollEvents {
    bits: u32,
}

impl PollEvents {
    pub const READ: Self = Self { bits: 1 << 0 };
    pub const WRITE: Self = Self { bits: 1 << 1 };
    pub const ERROR: Self = Self { bits: 1 << 2 };
    pub const LOCK: Self = Self { bits: 1 << 3 };

    pub const fn empty() -> Self {
        Self { bits: 0 }
    }

    pub const fn bits(self) -> u32 {
        self.bits
    }

    pub const fn contains(self, other: Self) -> bool {
        (self.bits & other.bits) == other.bits
    }

    pub const fn intersects(self, other: Self) -> bool {
        (self.bits & other.bits) != 0
    }

    pub const fn intersection(self, other: Self) -> Self {
        Self {
            bits: self.bits & other.bits,
        }
    }
}

impl BitOr for PollEvents {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self {
            bits: self.bits | rhs.bits,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MmapCachePolicy {
    Cached,
    Uncached,
    WriteThrough,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MmapRequest {
    pub offset: u64,
    pub length: u64,
    pub prot: u64,
    pub flags: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MmapKind {
    Buffered,
    DirectPhysical {
        physical_address: u64,
        cache_policy: MmapCachePolicy,
    },
    SharedPhysical {
        physical_pages: Arc<[u64]>,
        cache_policy: MmapCachePolicy,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MmapResponse {
    pub kind: MmapKind,
}

impl MmapResponse {
    pub const fn buffered() -> Self {
        Self {
            kind: MmapKind::Buffered,
        }
    }

    pub const fn direct_physical(physical_address: u64, cache_policy: MmapCachePolicy) -> Self {
        Self {
            kind: MmapKind::DirectPhysical {
                physical_address,
                cache_policy,
            },
        }
    }

    pub fn shared_physical(
        physical_pages: impl Into<Arc<[u64]>>,
        cache_policy: MmapCachePolicy,
    ) -> Self {
        Self {
            kind: MmapKind::SharedPhysical {
                physical_pages: physical_pages.into(),
                cache_policy,
            },
        }
    }
}

pub struct VfsFile {
    dentry: DentryRef,
    flags: OpenFlags,
    position: usize,
}

impl VfsFile {
    pub fn new(dentry: DentryRef, flags: OpenFlags) -> Self {
        dentry.inode().open();
        Self {
            dentry,
            flags,
            position: 0,
        }
    }

    pub fn from_inode(inode: NodeRef, flags: OpenFlags) -> Self {
        let name = String::from(inode.name());
        let dentry = Dentry::new(name, inode, None);
        Self::new(dentry, flags)
    }

    pub fn inode(&self) -> NodeRef {
        self.dentry.inode()
    }

    pub fn dentry(&self) -> DentryRef {
        self.dentry.clone()
    }

    pub fn flags(&self) -> OpenFlags {
        self.flags
    }

    pub fn set_flags(&mut self, flags: OpenFlags) {
        let preserved = self.flags.bits() & OpenFlags::DIRECTORY;
        let mutable = flags.bits() & !OpenFlags::DIRECTORY;
        self.flags = OpenFlags::from_bits(preserved | mutable);
    }

    pub fn position(&self) -> usize {
        self.position
    }

    pub fn set_position(&mut self, position: usize) {
        self.position = position;
    }

    pub fn read(&mut self, buffer: &mut [u8]) -> FsResult<usize> {
        if !self.flags.can_read() {
            return Err(FsError::InvalidInput);
        }
        match self.inode().read(self.position, buffer) {
            Ok(read) => {
                self.position = self.position.saturating_add(read);
                Ok(read)
            }
            Err(error) => Err(error),
        }
    }

    pub fn write(&mut self, buffer: &[u8]) -> FsResult<usize> {
        if !self.flags.can_write() {
            return Err(FsError::InvalidInput);
        }
        if self.flags.append() {
            self.position = self.inode().size();
        }
        match self.inode().write(self.position, buffer) {
            Ok(written) => {
                self.position = self.position.saturating_add(written);
                Ok(written)
            }
            Err(error) => Err(error),
        }
    }

    pub fn ioctl(&self, command: u64, argument: u64) -> FsResult<IoctlResponse> {
        self.inode().ioctl(command, argument)
    }

    pub fn advise(&self, offset: u64, len: u64, advice: FileAdvice) -> FsResult<()> {
        self.inode().advise(offset, len, advice)
    }

    pub fn fallocate(&self, mode: u32, offset: u64, len: u64) -> FsResult<()> {
        self.inode().fallocate(mode, offset, len)
    }

    pub fn poll(&self, events: PollEvents) -> FsResult<PollEvents> {
        self.inode().poll(events)
    }

    pub fn mmap(&self, request: MmapRequest) -> FsResult<MmapResponse> {
        self.inode().mmap(request)
    }

    pub fn wait_token(&self) -> u64 {
        self.inode().wait_token()
    }

    pub fn register_waiter(
        &self,
        events: PollEvents,
        listener: SharedWaitListener,
    ) -> FsResult<Option<u64>> {
        self.inode().register_waiter(events, listener)
    }

    pub fn unregister_waiter(&self, waiter_id: u64) -> FsResult<()> {
        self.inode().unregister_waiter(waiter_id)
    }
}

impl Drop for VfsFile {
    fn drop(&mut self) {
        self.dentry.inode().release();
    }
}

pub struct OpenFileDescription {
    id: u64,
    file: VfsFile,
}

impl OpenFileDescription {
    pub fn new(node: NodeRef, flags: OpenFlags) -> Self {
        Self {
            id: next_open_file_description_id(),
            file: VfsFile::from_inode(node, flags),
        }
    }

    pub fn from_dentry(dentry: DentryRef, flags: OpenFlags) -> Self {
        Self {
            id: next_open_file_description_id(),
            file: VfsFile::new(dentry, flags),
        }
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn node(&self) -> NodeRef {
        self.file.inode()
    }

    pub fn dentry(&self) -> DentryRef {
        self.file.dentry()
    }

    pub fn flags(&self) -> OpenFlags {
        self.file.flags()
    }

    pub fn set_flags(&mut self, flags: OpenFlags) {
        self.file.set_flags(flags);
    }

    pub fn position(&self) -> usize {
        self.file.position()
    }

    pub fn set_position(&mut self, position: usize) {
        self.file.set_position(position);
    }

    pub fn read(&mut self, buffer: &mut [u8]) -> FsResult<usize> {
        self.file.read(buffer)
    }

    pub fn write(&mut self, buffer: &[u8]) -> FsResult<usize> {
        self.file.write(buffer)
    }

    pub fn ioctl(&self, command: u64, argument: u64) -> FsResult<IoctlResponse> {
        self.file.ioctl(command, argument)
    }

    pub fn advise(&self, offset: u64, len: u64, advice: FileAdvice) -> FsResult<()> {
        self.file.advise(offset, len, advice)
    }

    pub fn fallocate(&self, mode: u32, offset: u64, len: u64) -> FsResult<()> {
        self.file.fallocate(mode, offset, len)
    }

    pub fn flock(&self, operation: FlockOperation) -> FsResult<()> {
        self.file.inode().flock(self.id, operation)
    }

    pub fn poll(&self, events: PollEvents) -> FsResult<PollEvents> {
        self.file.poll(events)
    }

    pub fn mmap(&self, request: MmapRequest) -> FsResult<MmapResponse> {
        self.file.mmap(request)
    }

    pub fn wait_token(&self) -> u64 {
        self.file.wait_token()
    }

    pub fn register_waiter(
        &self,
        events: PollEvents,
        listener: SharedWaitListener,
    ) -> FsResult<Option<u64>> {
        if events.contains(PollEvents::LOCK) {
            return Ok(Some(
                FLOCK_WAITER_TAG | self.file.inode().register_flock_waiter(listener),
            ));
        }
        self.file.register_waiter(events, listener)
    }

    pub fn unregister_waiter(&self, waiter_id: u64) -> FsResult<()> {
        if (waiter_id & FLOCK_WAITER_TAG) != 0 {
            self.file
                .inode()
                .unregister_flock_waiter(waiter_id & !FLOCK_WAITER_TAG);
            return Ok(());
        }
        self.file.unregister_waiter(waiter_id)
    }
}

impl Drop for OpenFileDescription {
    fn drop(&mut self) {
        let _ = self.file.inode().flock(self.id, FlockOperation::Unlock);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlockOperation {
    Shared,
    Exclusive,
    Unlock,
}

#[derive(Debug, Default)]
pub(crate) struct FlockState {
    pub(crate) shared: BTreeSet<u64>,
    pub(crate) exclusive: Option<u64>,
}

const FLOCK_WAITER_TAG: u64 = 1 << 63;
static NEXT_OPEN_FILE_DESCRIPTION_ID: AtomicU64 = AtomicU64::new(1);

fn next_open_file_description_id() -> u64 {
    NEXT_OPEN_FILE_DESCRIPTION_ID.fetch_add(1, Ordering::AcqRel)
}

pub type SharedOpenFile = Arc<SpinLock<OpenFileDescription>>;
