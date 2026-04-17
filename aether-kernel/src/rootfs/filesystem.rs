extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;

use aether_fs::ext::{self, EXT_SUPER_MAGIC};
use aether_tmpfs as tmpfs;
use aether_vfs::{
    DirectoryEntry, FileOperations, FsResult, Inode, InodeOperations, NodeKind, NodeMetadata,
    NodeRef,
};

use crate::errno::{SysErr, SysResult};
use crate::fs::LinuxStatFs;

const DEFAULT_BLOCK_SIZE: u64 = 4096;
const DEFAULT_NAME_LEN: u64 = 255;

pub const TMPFS_MAGIC: u64 = 0x0102_1994;
pub const PROC_SUPER_MAGIC: u64 = 0x0000_9fa0;
pub const SYSFS_MAGIC: u64 = 0x6265_6572;
pub const RAMFS_MAGIC: u64 = 0x8584_58f6;
pub const BIND_MAGIC: u64 = 0;

#[derive(Clone)]
pub struct MountRequest {
    pub target_name: String,
    pub source: Option<NodeRef>,
}

pub struct FileSystemMount {
    pub root: NodeRef,
    pub statfs: LinuxStatFs,
}

pub trait KernelFileSystem: Send + Sync {
    fn fstype(&self) -> &'static str;
    fn magic(&self) -> u64;
    fn mount(&self, request: &MountRequest) -> SysResult<FileSystemMount>;
}

#[derive(Clone)]
pub struct MountedNode {
    node: NodeRef,
    device_id: u64,
    statfs: LinuxStatFs,
}

impl MountedNode {
    pub fn new(
        node: NodeRef,
        _filesystem: Arc<dyn KernelFileSystem>,
        device_id: u64,
        statfs: LinuxStatFs,
    ) -> Self {
        Self {
            node,
            device_id,
            statfs: statfs.with_device_id(device_id),
        }
    }

    pub fn node(&self) -> NodeRef {
        Inode::new(Arc::new(MountedVfsNode {
            node: self.node.clone(),
            device_id: self.device_id,
        }))
    }

    pub fn device_id(&self) -> u64 {
        self.device_id
    }

    pub fn statfs(&self) -> LinuxStatFs {
        self.statfs
    }
}

struct MountedVfsNode {
    node: NodeRef,
    device_id: u64,
}

impl MountedVfsNode {
    fn wrap(&self, node: NodeRef) -> NodeRef {
        Inode::new(Arc::new(Self {
            node,
            device_id: self.device_id,
        }))
    }
}

impl InodeOperations for MountedVfsNode {
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn name(&self) -> &str {
        self.node.name()
    }

    fn kind(&self) -> NodeKind {
        self.node.kind()
    }

    fn lookup(&self, name: &str) -> Option<NodeRef> {
        self.node.lookup(name).map(|node| self.wrap(node))
    }

    fn insert_child(&self, name: String, node: NodeRef) -> FsResult<()> {
        self.node.insert_child(name, node)
    }

    fn remove_child(&self, name: &str, remove_directory: bool) -> FsResult<()> {
        self.node.remove_child(name, remove_directory)
    }

    fn rename_child(
        &self,
        old_name: &str,
        new_parent: &NodeRef,
        new_name: String,
        replace: bool,
    ) -> FsResult<()> {
        self.node
            .rename_child(old_name, new_parent, new_name, replace)
    }

    fn create_file(&self, name: String, mode: u32) -> FsResult<NodeRef> {
        self.node
            .create_file(name, mode)
            .map(|node| self.wrap(node))
    }

    fn create_dir(&self, name: String, mode: u32) -> FsResult<NodeRef> {
        self.node.create_dir(name, mode).map(|node| self.wrap(node))
    }

    fn create_symlink(&self, name: String, target: String, mode: u32) -> FsResult<NodeRef> {
        self.node
            .create_symlink(name, target, mode)
            .map(|node| self.wrap(node))
    }

    fn create_socket(&self, name: String, mode: u32) -> FsResult<NodeRef> {
        self.node
            .create_socket(name, mode)
            .map(|node| self.wrap(node))
    }

    fn entries(&self) -> alloc::vec::Vec<DirectoryEntry> {
        self.node.entries()
    }

    fn file_ops(&self) -> Option<&dyn FileOperations> {
        self.node.file()
    }

    fn symlink_target(&self) -> Option<&str> {
        self.node.symlink_target()
    }

    fn device_numbers(&self) -> Option<(u32, u32)> {
        self.node.device_numbers()
    }

    fn mode(&self) -> Option<u32> {
        self.node.mode()
    }

    fn set_mode(&self, mode: u32) -> FsResult<()> {
        self.node.set_mode(mode)
    }

    fn set_owner(&self, uid: u32, gid: u32) -> FsResult<()> {
        self.node.set_owner(uid, gid)
    }

    fn metadata(&self) -> NodeMetadata {
        self.node.metadata().with_device_id(self.device_id)
    }
}

pub struct FileSystemRegistry {
    entries: BTreeMap<String, Arc<dyn KernelFileSystem>>,
}

impl FileSystemRegistry {
    pub fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }

    pub fn register(&mut self, filesystem: Arc<dyn KernelFileSystem>) {
        self.entries
            .insert(String::from(filesystem.fstype()), filesystem);
    }

    pub fn get(&self, fstype: &str) -> Option<Arc<dyn KernelFileSystem>> {
        self.entries.get(fstype).cloned()
    }

    pub fn types(&self) -> alloc::vec::Vec<&str> {
        self.entries.keys().map(String::as_str).collect()
    }

    pub fn mount_with_device(
        &self,
        fstype: &str,
        request: &MountRequest,
        device_id: u64,
    ) -> SysResult<MountedNode> {
        let filesystem = self.get(fstype).ok_or(SysErr::NoSys)?;
        let mount = filesystem.mount(request)?;
        Ok(MountedNode::new(
            mount.root,
            filesystem,
            device_id,
            mount.statfs,
        ))
    }
}

pub struct StaticDirectoryFs {
    fstype: &'static str,
    magic: u64,
    root: NodeRef,
}

impl StaticDirectoryFs {
    pub fn new(fstype: &'static str, magic: u64, root: NodeRef) -> Self {
        Self {
            fstype,
            magic,
            root,
        }
    }
}

impl KernelFileSystem for StaticDirectoryFs {
    fn fstype(&self) -> &'static str {
        self.fstype
    }

    fn magic(&self) -> u64 {
        self.magic
    }

    fn mount(&self, _request: &MountRequest) -> SysResult<FileSystemMount> {
        Ok(FileSystemMount {
            root: self.root.clone(),
            statfs: LinuxStatFs::new(self.magic, DEFAULT_BLOCK_SIZE, DEFAULT_NAME_LEN),
        })
    }
}

pub struct TmpFs;

impl KernelFileSystem for TmpFs {
    fn fstype(&self) -> &'static str {
        "tmpfs"
    }

    fn magic(&self) -> u64 {
        TMPFS_MAGIC
    }

    fn mount(&self, request: &MountRequest) -> SysResult<FileSystemMount> {
        Ok(FileSystemMount {
            root: tmpfs::directory(request.target_name.as_str()),
            statfs: LinuxStatFs::new(self.magic(), DEFAULT_BLOCK_SIZE, DEFAULT_NAME_LEN),
        })
    }
}

pub struct BindFs;

impl KernelFileSystem for BindFs {
    fn fstype(&self) -> &'static str {
        "bind"
    }

    fn magic(&self) -> u64 {
        BIND_MAGIC
    }

    fn mount(&self, _request: &MountRequest) -> SysResult<FileSystemMount> {
        Err(SysErr::Inval)
    }
}

pub struct ExtFileSystem {
    fstype: &'static str,
}

impl ExtFileSystem {
    pub const fn new(fstype: &'static str) -> Self {
        Self { fstype }
    }
}

impl KernelFileSystem for ExtFileSystem {
    fn fstype(&self) -> &'static str {
        self.fstype
    }

    fn magic(&self) -> u64 {
        EXT_SUPER_MAGIC
    }

    fn mount(&self, request: &MountRequest) -> SysResult<FileSystemMount> {
        let source = request.source.as_ref().ok_or(SysErr::Inval)?;
        let mounted =
            ext::mount_from_source(source, request.target_name.as_str()).map_err(SysErr::from)?;
        let stat = mounted.stat();
        Ok(FileSystemMount {
            root: mounted.root(),
            statfs: LinuxStatFs {
                f_type: self.magic(),
                f_bsize: stat.block_size,
                f_blocks: stat.blocks,
                f_bfree: stat.free_blocks,
                f_bavail: stat.free_blocks,
                f_files: stat.files,
                f_ffree: stat.free_inodes,
                f_fsid: [0; 2],
                f_namelen: stat.name_len,
                f_frsize: stat.block_size,
                f_flags: 0,
                f_spare: [0; 4],
            },
        })
    }
}
