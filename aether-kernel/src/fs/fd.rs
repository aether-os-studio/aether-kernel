extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::sync::Arc;

use aether_frame::libs::spin::SpinLock;
use aether_vfs::{NodeRef, OpenFileDescription, OpenFlags, SharedOpenFile};

use crate::fs::FileSystemIdentity;
use crate::rootfs::FsLocation;

#[derive(Clone)]
pub struct FileDescriptor {
    pub file: SharedOpenFile,
    pub filesystem: FileSystemIdentity,
    pub location: Option<FsLocation>,
    pub cloexec: bool,
}

#[derive(Clone, Default)]
pub struct FdTable {
    entries: BTreeMap<u32, FileDescriptor>,
}

impl FdTable {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn new_with_stdio(
        stdin: NodeRef,
        stdout: NodeRef,
        stderr: NodeRef,
        filesystem: FileSystemIdentity,
    ) -> Self {
        let mut table = Self::empty();
        table.entries.insert(
            0,
            Self::descriptor(stdin, OpenFlags::from_bits(OpenFlags::READ), filesystem),
        );
        table.entries.insert(
            1,
            Self::descriptor(stdout, OpenFlags::from_bits(OpenFlags::WRITE), filesystem),
        );
        table.entries.insert(
            2,
            Self::descriptor(stderr, OpenFlags::from_bits(OpenFlags::WRITE), filesystem),
        );
        table
    }

    pub fn insert_node(
        &mut self,
        node: NodeRef,
        flags: OpenFlags,
        filesystem: FileSystemIdentity,
        location: Option<FsLocation>,
        cloexec: bool,
    ) -> u32 {
        self.insert(
            FileDescriptor {
                file: Arc::new(SpinLock::new(OpenFileDescription::new(node, flags))),
                filesystem,
                location,
                cloexec,
            },
            0,
        )
    }

    pub fn insert(&mut self, descriptor: FileDescriptor, min_fd: u32) -> u32 {
        let mut fd = min_fd;
        while self.entries.contains_key(&fd) {
            fd = fd.saturating_add(1);
        }
        self.entries.insert(fd, descriptor);
        fd
    }

    pub fn get(&self, fd: u32) -> Option<&FileDescriptor> {
        self.entries.get(&fd)
    }

    pub fn get_mut(&mut self, fd: u32) -> Option<&mut FileDescriptor> {
        self.entries.get_mut(&fd)
    }

    pub fn entries(&self) -> impl Iterator<Item = (&u32, &FileDescriptor)> {
        self.entries.iter()
    }

    pub fn insert_at(&mut self, fd: u32, descriptor: FileDescriptor) {
        self.entries.insert(fd, descriptor);
    }

    pub fn close(&mut self, fd: u32) -> bool {
        self.entries.remove(&fd).is_some()
    }

    pub fn duplicate(&mut self, fd: u32, min_fd: u32, cloexec: bool) -> Option<u32> {
        let mut duplicate = self.get(fd)?.clone();
        duplicate.cloexec = cloexec;
        Some(self.insert(duplicate, min_fd))
    }

    pub fn duplicate_to(&mut self, fd: u32, newfd: u32, cloexec: bool) -> Option<u32> {
        let mut duplicate = self.get(fd)?.clone();
        duplicate.cloexec = cloexec;
        self.entries.insert(newfd, duplicate);
        Some(newfd)
    }

    pub fn close_range(&mut self, first: u32, last: u32) {
        if first > last {
            return;
        }

        let mut tail = self.entries.split_off(&first);
        let mut keep = if last == u32::MAX {
            BTreeMap::new()
        } else {
            tail.split_off(&last.saturating_add(1))
        };
        self.entries.append(&mut keep);
    }

    pub fn close_cloexec(&mut self) {
        self.entries.retain(|_, descriptor| !descriptor.cloexec);
    }

    pub fn set_cloexec_range(&mut self, first: u32, last: u32) {
        if first > last {
            return;
        }

        for (_, descriptor) in self.entries.range_mut(first..=last) {
            descriptor.cloexec = true;
        }
    }

    fn descriptor(
        node: NodeRef,
        flags: OpenFlags,
        filesystem: FileSystemIdentity,
    ) -> FileDescriptor {
        FileDescriptor {
            file: Arc::new(SpinLock::new(OpenFileDescription::new(node, flags))),
            filesystem,
            location: None,
            cloexec: false,
        }
    }
}

pub fn linux_open_flags(raw: u64) -> OpenFlags {
    const O_ACCMODE: u64 = 0o3;
    const O_WRONLY: u64 = 0o1;
    const O_RDWR: u64 = 0o2;
    const O_APPEND: u64 = 0o2000;
    const O_NONBLOCK: u64 = 0o4000;
    const O_DIRECTORY: u64 = 0o200000;

    let mut bits = match raw & O_ACCMODE {
        O_WRONLY => OpenFlags::WRITE,
        O_RDWR => OpenFlags::READ | OpenFlags::WRITE,
        _ => OpenFlags::READ,
    };

    if (raw & O_APPEND) != 0 {
        bits |= OpenFlags::APPEND;
    }
    if (raw & O_NONBLOCK) != 0 {
        bits |= OpenFlags::NONBLOCK;
    }
    if (raw & O_DIRECTORY) != 0 {
        bits |= OpenFlags::DIRECTORY;
    }

    OpenFlags::from_bits(bits)
}

pub fn linux_status_flags(flags: OpenFlags) -> u64 {
    const O_WRONLY: u64 = 0o1;
    const O_RDWR: u64 = 0o2;
    const O_APPEND: u64 = 0o2000;
    const O_NONBLOCK: u64 = 0o4000;
    const O_DIRECTORY: u64 = 0o200000;

    let mut raw = if flags.can_write() {
        if flags.can_read() { O_RDWR } else { O_WRONLY }
    } else {
        0
    };

    if flags.append() {
        raw |= O_APPEND;
    }
    if flags.nonblock() {
        raw |= O_NONBLOCK;
    }
    if flags.directory() {
        raw |= O_DIRECTORY;
    }

    raw
}
