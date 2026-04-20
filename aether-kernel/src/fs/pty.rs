extern crate alloc;

use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::str::FromStr;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use aether_frame::libs::spin::{LocalIrqDisabled, SpinLock};
use aether_terminal::{LinuxWinSize, TtyBackend, TtyFile};
use aether_vfs::{
    DirectoryEntry, FileOperations, FsError, FsResult, Inode, InodeOperations, IoctlResponse,
    NodeKind, NodeMetadata, NodeRef, OpenFlags, PollEvents, SharedWaitListener, WaitQueue,
};

use crate::rootfs::{FileSystemMount, KernelFileSystem, MountRequest};

const DEVPTS_SUPER_MAGIC: u64 = 0x1cd1;
const DEVPTS_BLOCK_SIZE: u64 = 4096;
const DEVPTS_NAME_LEN: u64 = 255;
const PTMX_MAJOR: u32 = 5;
const PTMX_MINOR: u32 = 2;
const UNIX98_PTY_SLAVE_MAJOR: u32 = 136;
const TCXONC: u64 = 0x540a;
const TCSBRK: u64 = 0x5409;
const TCSBRKP: u64 = 0x5425;
const OPOST: u32 = 0o000001;
const ONLCR: u32 = 0o000004;
const IXON: u32 = 0o002000;
const VSTART: usize = 8;
const VSTOP: usize = 9;
const TCOOFF: i32 = 0;
const TCOON: i32 = 1;
const TCIOFF: i32 = 2;
const TCION: i32 = 3;

#[derive(Clone)]
struct PtyShared {
    id: u32,
    manager: Weak<PtyManager>,
    locked: Arc<AtomicBool>,
    master_alive: Arc<AtomicBool>,
    slave_open_count: Arc<AtomicU32>,
    master_output_stopped: Arc<AtomicBool>,
    slave_output_stopped: Arc<AtomicBool>,
    master_buffer: Arc<SpinLock<VecDeque<u8>, LocalIrqDisabled>>,
    master_version: Arc<AtomicU64>,
    master_waiters: Arc<WaitQueue>,
}

impl PtyShared {
    fn new(id: u32, manager: &Arc<PtyManager>) -> Self {
        Self {
            id,
            manager: Arc::downgrade(manager),
            locked: Arc::new(AtomicBool::new(true)),
            master_alive: Arc::new(AtomicBool::new(true)),
            slave_open_count: Arc::new(AtomicU32::new(0)),
            master_output_stopped: Arc::new(AtomicBool::new(false)),
            slave_output_stopped: Arc::new(AtomicBool::new(false)),
            master_buffer: Arc::new(SpinLock::new(VecDeque::new())),
            master_version: Arc::new(AtomicU64::new(1)),
            master_waiters: Arc::new(WaitQueue::new()),
        }
    }

    fn bump_master_waiters(&self, events: PollEvents) {
        let _ = self.master_version.fetch_add(1, Ordering::AcqRel);
        self.master_waiters.notify(events);
    }

    fn enqueue_master_bytes(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }

        let mut queue = self.master_buffer.lock();
        for byte in bytes {
            queue.push_back(*byte);
        }
        drop(queue);

        self.bump_master_waiters(PollEvents::READ);
    }

    fn dequeue_master_bytes(&self, buffer: &mut [u8]) -> usize {
        let mut queue = self.master_buffer.lock();
        let mut read = 0usize;
        while read < buffer.len() {
            let Some(byte) = queue.pop_front() else {
                break;
            };
            buffer[read] = byte;
            read += 1;
        }
        read
    }

    fn master_readable(&self) -> bool {
        !self.master_buffer.lock().is_empty()
    }

    fn master_alive(&self) -> bool {
        self.master_alive.load(Ordering::Acquire)
    }

    fn master_output_stopped(&self) -> bool {
        self.master_output_stopped.load(Ordering::Acquire)
    }

    fn slave_output_stopped(&self) -> bool {
        self.slave_output_stopped.load(Ordering::Acquire)
    }

    fn set_master_output_stopped(&self, stopped: bool) {
        let previous = self.master_output_stopped.swap(stopped, Ordering::AcqRel);
        if previous != stopped {
            self.bump_master_waiters(PollEvents::WRITE);
        }
    }

    fn set_slave_output_stopped(&self, stopped: bool) -> bool {
        self.slave_output_stopped.swap(stopped, Ordering::AcqRel) != stopped
    }

    fn slave_connected(&self) -> bool {
        self.slave_open_count.load(Ordering::Acquire) != 0
    }

    fn open_slave(&self) {
        if self.slave_open_count.fetch_add(1, Ordering::AcqRel) == 0 {
            self.bump_master_waiters(PollEvents::WRITE | PollEvents::HUP | PollEvents::RDHUP);
        }
    }

    fn close_slave(&self) {
        let mut current = self.slave_open_count.load(Ordering::Acquire);
        loop {
            if current == 0 {
                return;
            }

            match self.slave_open_count.compare_exchange(
                current,
                current - 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(previous) => {
                    if previous == 1 {
                        self.bump_master_waiters(
                            PollEvents::WRITE
                                | PollEvents::ERROR
                                | PollEvents::HUP
                                | PollEvents::RDHUP,
                        );
                        self.cleanup_if_unused();
                    }
                    return;
                }
                Err(observed) => current = observed,
            }
        }
    }

    fn close_master(&self) {
        if self.master_alive.swap(false, Ordering::AcqRel) {
            self.cleanup_if_unused();
        }
    }

    fn cleanup_if_unused(&self) {
        if self.master_alive() || self.slave_connected() {
            return;
        }

        if let Some(manager) = self.manager.upgrade() {
            manager.remove_pty(self.id);
        }
    }
}

fn translate_slave_output(termios: aether_terminal::LinuxTermios, bytes: &[u8]) -> Vec<u8> {
    // TODO: Linux c_oflag handling is broader than OPOST|ONLCR. Extend this once
    // userspace needs more PTY output transformations.
    if bytes.is_empty() || (termios.c_oflag & OPOST) == 0 || (termios.c_oflag & ONLCR) == 0 {
        return bytes.to_vec();
    }

    let mut translated = Vec::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        if *byte == b'\n' {
            translated.push(b'\r');
        }
        translated.push(*byte);
    }
    translated
}

struct PtySlaveBackend {
    shared: PtyShared,
}

impl TtyBackend for PtySlaveBackend {
    fn write_bytes(&self, bytes: &[u8]) {
        self.shared.enqueue_master_bytes(bytes);
    }

    fn poll_ready(&self, events: PollEvents) -> PollEvents {
        let mut ready = PollEvents::empty();
        if events.contains(PollEvents::WRITE)
            && self.shared.master_alive()
            && !self.shared.slave_output_stopped()
        {
            ready = ready | PollEvents::WRITE;
        }
        ready
    }
}

struct PtyEntry {
    slave_node: NodeRef,
}

pub struct DevPtsSlaveFile {
    shared: PtyShared,
    tty: Arc<TtyFile>,
}

impl DevPtsSlaveFile {
    pub fn tty(&self) -> &TtyFile {
        self.tty.as_ref()
    }

    fn apply_tcxonc(&self, action: i32) -> FsResult<()> {
        match action {
            TCOOFF => {
                if self.shared.set_slave_output_stopped(true) {
                    self.tty.notify_events(PollEvents::WRITE);
                }
                Ok(())
            }
            TCOON => {
                if self.shared.set_slave_output_stopped(false) {
                    self.tty.notify_events(PollEvents::WRITE);
                }
                Ok(())
            }
            TCIOFF | TCION => {
                if !self.shared.master_alive() {
                    return Ok(());
                }
                let termios = self.tty.termios();
                if (termios.c_iflag & IXON) != 0 {
                    let index = if action == TCIOFF { VSTOP } else { VSTART };
                    self.shared.enqueue_master_bytes(&[termios.c_cc[index]]);
                }
                Ok(())
            }
            _ => Err(FsError::InvalidInput),
        }
    }
}

impl FileOperations for DevPtsSlaveFile {
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn open(&self) {
        self.shared.open_slave();
    }

    fn release(&self) {
        self.shared.close_slave();
    }

    fn read(&self, offset: usize, buffer: &mut [u8]) -> FsResult<usize> {
        match self.tty.read(offset, buffer) {
            Err(FsError::WouldBlock) if !self.shared.master_alive() => Ok(0),
            other => other,
        }
    }

    fn write(&self, offset: usize, buffer: &[u8]) -> FsResult<usize> {
        let _ = offset;
        if buffer.is_empty() {
            return Ok(0);
        }
        if !self.shared.master_alive() {
            return Err(FsError::Io);
        }
        if self.shared.slave_output_stopped() {
            return Err(FsError::WouldBlock);
        }

        let translated = translate_slave_output(self.tty.termios(), buffer);
        self.shared.enqueue_master_bytes(&translated);
        Ok(buffer.len())
    }

    fn ioctl(&self, command: u64, argument: u64) -> FsResult<IoctlResponse> {
        match command {
            TCXONC => {
                self.apply_tcxonc(argument as i32)?;
                Ok(IoctlResponse::success())
            }
            // TODO: PTY break timing/propagation is still not implemented.
            TCSBRK | TCSBRKP => Ok(IoctlResponse::success()),
            _ => self.tty.ioctl(command, argument),
        }
    }

    fn poll(&self, events: PollEvents) -> FsResult<PollEvents> {
        let tty_ready = self.tty.poll(events)?;
        let mut ready = PollEvents::empty();

        if events.contains(PollEvents::READ) && tty_ready.contains(PollEvents::READ) {
            ready = ready | PollEvents::READ;
        }
        if events.contains(PollEvents::WRITE)
            && self.shared.master_alive()
            && !self.shared.slave_output_stopped()
            && tty_ready.contains(PollEvents::WRITE)
        {
            ready = ready | PollEvents::WRITE;
        }
        if !self.shared.master_alive() {
            ready = ready | PollEvents::ERROR | PollEvents::HUP | PollEvents::RDHUP;
        }

        Ok(ready)
    }

    fn wait_token(&self) -> u64 {
        self.tty.wait_token()
    }

    fn register_waiter(
        &self,
        events: PollEvents,
        listener: SharedWaitListener,
    ) -> FsResult<Option<u64>> {
        self.tty.register_waiter(events, listener)
    }

    fn unregister_waiter(&self, waiter_id: u64) -> FsResult<()> {
        self.tty.unregister_waiter(waiter_id)
    }
}

pub struct PtyManager {
    next_id: AtomicU32,
    entries: SpinLock<BTreeMap<u32, PtyEntry>, LocalIrqDisabled>,
}

impl PtyManager {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            next_id: AtomicU32::new(0),
            entries: SpinLock::new(BTreeMap::new()),
        })
    }

    fn next_pty_id(&self) -> u32 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    fn allocate_master(self: &Arc<Self>) -> Arc<PtmxMasterFile> {
        let id = self.next_pty_id();
        let shared = PtyShared::new(id, self);
        let backend: Arc<dyn TtyBackend> = Arc::new(PtySlaveBackend {
            shared: shared.clone(),
        });
        let slave = Arc::new(TtyFile::new(
            backend,
            LinuxWinSize {
                ws_row: 25,
                ws_col: 80,
                ws_xpixel: 0,
                ws_ypixel: 0,
            },
        ));
        let slave_node: NodeRef = Inode::new(Arc::new(DevPtsSlaveNode {
            id,
            name: alloc::format!("{id}"),
            shared: shared.clone(),
            slave: slave.clone(),
            metadata: SpinLock::new(NodeMetadata::device(0o020620, UNIX98_PTY_SLAVE_MAJOR, id)),
        }));

        self.entries.lock().insert(id, PtyEntry { slave_node });
        Arc::new(PtmxMasterFile { shared, slave })
    }

    fn lookup_slave(&self, id: u32) -> Option<NodeRef> {
        self.entries
            .lock()
            .get(&id)
            .map(|entry| entry.slave_node.clone())
    }

    fn remove_pty(&self, id: u32) {
        let _ = self.entries.lock().remove(&id);
    }

    fn entries(&self) -> alloc::vec::Vec<DirectoryEntry> {
        let mut entries = alloc::vec![DirectoryEntry {
            name: String::from("ptmx"),
            kind: NodeKind::CharDevice,
        }];
        entries.extend(self.entries.lock().keys().map(|id| DirectoryEntry {
            name: alloc::format!("{id}"),
            kind: NodeKind::CharDevice,
        }));
        entries
    }
}

struct PtmxNode {
    manager: Arc<PtyManager>,
    metadata: SpinLock<NodeMetadata>,
}

impl InodeOperations for PtmxNode {
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn name(&self) -> &str {
        "ptmx"
    }

    fn kind(&self) -> NodeKind {
        NodeKind::CharDevice
    }

    fn open_file(&self, _flags: OpenFlags) -> FsResult<Arc<dyn FileOperations>> {
        Ok(self.manager.allocate_master())
    }

    fn device_numbers(&self) -> Option<(u32, u32)> {
        Some((PTMX_MAJOR, PTMX_MINOR))
    }

    fn mode(&self) -> Option<u32> {
        Some(self.metadata.lock().mode)
    }

    fn metadata(&self) -> NodeMetadata {
        *self.metadata.lock()
    }
}

struct DevPtsSlaveNode {
    id: u32,
    name: String,
    shared: PtyShared,
    slave: Arc<TtyFile>,
    metadata: SpinLock<NodeMetadata>,
}

impl InodeOperations for DevPtsSlaveNode {
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> NodeKind {
        NodeKind::CharDevice
    }

    fn file_ops(&self) -> Option<&dyn FileOperations> {
        Some(self.slave.as_ref())
    }

    fn open_file(&self, _flags: OpenFlags) -> FsResult<Arc<dyn FileOperations>> {
        if self.shared.locked.load(Ordering::Acquire) {
            return Err(FsError::Io);
        }

        Ok(Arc::new(DevPtsSlaveFile {
            shared: self.shared.clone(),
            tty: self.slave.clone(),
        }))
    }

    fn device_numbers(&self) -> Option<(u32, u32)> {
        Some((UNIX98_PTY_SLAVE_MAJOR, self.id))
    }

    fn mode(&self) -> Option<u32> {
        Some(self.metadata.lock().mode)
    }

    fn metadata(&self) -> NodeMetadata {
        *self.metadata.lock()
    }
}

struct DevPtsRootNode {
    name: String,
    manager: Arc<PtyManager>,
    ptmx_node: NodeRef,
    metadata: SpinLock<NodeMetadata>,
}

impl InodeOperations for DevPtsRootNode {
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> NodeKind {
        NodeKind::Directory
    }

    fn lookup(&self, name: &str) -> Option<NodeRef> {
        if name == "ptmx" {
            return Some(self.ptmx_node.clone());
        }

        let id = u32::from_str(name).ok()?;
        self.manager.lookup_slave(id)
    }

    fn entries(&self) -> alloc::vec::Vec<DirectoryEntry> {
        self.manager.entries()
    }

    fn mode(&self) -> Option<u32> {
        Some(self.metadata.lock().mode)
    }

    fn metadata(&self) -> NodeMetadata {
        *self.metadata.lock()
    }
}

pub struct PtmxMasterFile {
    shared: PtyShared,
    slave: Arc<TtyFile>,
}

impl PtmxMasterFile {
    pub fn pty_number(&self) -> u32 {
        self.shared.id
    }

    pub fn locked(&self) -> bool {
        self.shared.locked.load(Ordering::Acquire)
    }

    pub fn set_locked(&self, locked: bool) {
        self.shared.locked.store(locked, Ordering::Release);
    }

    pub fn slave(&self) -> &TtyFile {
        self.slave.as_ref()
    }

    pub fn peer_node(&self) -> Option<NodeRef> {
        self.shared
            .manager
            .upgrade()
            .and_then(|manager| manager.lookup_slave(self.shared.id))
    }

    fn apply_tcxonc(&self, action: i32) -> FsResult<()> {
        match action {
            TCOOFF => {
                self.shared.set_master_output_stopped(true);
                Ok(())
            }
            TCOON => {
                self.shared.set_master_output_stopped(false);
                Ok(())
            }
            TCIOFF | TCION => {
                if !self.shared.slave_connected() {
                    return Ok(());
                }
                let termios = self.slave.termios();
                if (termios.c_iflag & IXON) != 0 {
                    let index = if action == TCIOFF { VSTOP } else { VSTART };
                    self.slave.receive_bytes(&[termios.c_cc[index]]);
                }
                Ok(())
            }
            _ => Err(FsError::InvalidInput),
        }
    }
}

impl FileOperations for PtmxMasterFile {
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn read(&self, _offset: usize, buffer: &mut [u8]) -> FsResult<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }

        let read = self.shared.dequeue_master_bytes(buffer);
        if read == 0 {
            if !self.shared.slave_connected() {
                return Ok(0);
            }
            return Err(FsError::WouldBlock);
        }

        Ok(read)
    }

    fn write(&self, _offset: usize, buffer: &[u8]) -> FsResult<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        if self.shared.master_output_stopped() {
            return Err(FsError::WouldBlock);
        }
        if !self.shared.slave_connected() {
            return Err(FsError::Io);
        }

        self.slave.receive_bytes(buffer);
        Ok(buffer.len())
    }

    fn release(&self) {
        self.shared.close_master();
        self.slave
            .notify_events(PollEvents::ERROR | PollEvents::HUP | PollEvents::RDHUP);
        self.shared
            .master_waiters
            .notify(PollEvents::ERROR | PollEvents::HUP | PollEvents::RDHUP);
    }

    fn ioctl(&self, command: u64, argument: u64) -> FsResult<IoctlResponse> {
        match command {
            TCXONC => {
                self.apply_tcxonc(argument as i32)?;
                Ok(IoctlResponse::success())
            }
            // TODO: PTY break timing/propagation is still not implemented.
            TCSBRK | TCSBRKP => Ok(IoctlResponse::success()),
            _ => Err(FsError::Unsupported),
        }
    }

    fn poll(&self, events: PollEvents) -> FsResult<PollEvents> {
        let mut ready = PollEvents::empty();

        if events.contains(PollEvents::READ) && self.shared.master_readable() {
            ready = ready | PollEvents::READ;
        }
        if events.contains(PollEvents::WRITE)
            && self.shared.slave_connected()
            && !self.shared.master_output_stopped()
        {
            ready = ready | PollEvents::WRITE;
        }
        if !self.shared.slave_connected() {
            ready = ready | PollEvents::ERROR | PollEvents::HUP | PollEvents::RDHUP;
        }

        Ok(ready)
    }

    fn wait_token(&self) -> u64 {
        self.shared.master_version.load(Ordering::Acquire)
    }

    fn register_waiter(
        &self,
        events: PollEvents,
        listener: SharedWaitListener,
    ) -> FsResult<Option<u64>> {
        Ok(Some(self.shared.master_waiters.register(events, listener)))
    }

    fn unregister_waiter(&self, waiter_id: u64) -> FsResult<()> {
        let _ = self.shared.master_waiters.unregister(waiter_id);
        Ok(())
    }
}

pub struct DevPtsFs {
    manager: Arc<PtyManager>,
    ptmx_node: NodeRef,
}

impl DevPtsFs {
    pub fn new() -> Arc<Self> {
        let manager = PtyManager::new();
        Arc::new(Self {
            ptmx_node: Inode::new(Arc::new(PtmxNode {
                manager: manager.clone(),
                metadata: SpinLock::new(NodeMetadata::device(0o020666, PTMX_MAJOR, PTMX_MINOR)),
            })),
            manager,
        })
    }

    pub fn ptmx_node(&self) -> NodeRef {
        self.ptmx_node.clone()
    }
}

impl KernelFileSystem for DevPtsFs {
    fn fstype(&self) -> &'static str {
        "devpts"
    }

    fn magic(&self) -> u64 {
        DEVPTS_SUPER_MAGIC
    }

    fn mount(&self, request: &MountRequest) -> crate::errno::SysResult<FileSystemMount> {
        Ok(FileSystemMount {
            root: Inode::new(Arc::new(DevPtsRootNode {
                name: request.target_name.clone(),
                manager: self.manager.clone(),
                ptmx_node: self.ptmx_node.clone(),
                metadata: SpinLock::new(NodeMetadata::directory(0o040755)),
            })),
            statfs: crate::fs::LinuxStatFs::new(self.magic(), DEVPTS_BLOCK_SIZE, DEVPTS_NAME_LEN),
        })
    }
}
