extern crate alloc;

use aether_frame::libs::mutex::{Mutex, MutexGuard};
use aether_frame::libs::spin::SpinLock;
use aether_vfs::{FsError, FsResult, Inode, NodeKind, NodeMetadata, NodeRef, NodeTimestamp};
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use core::future::Future;
use core::time::Duration;
use ext4plus::prelude::{
    Dir, DirEntryName, Ext4, File as ExtFile, FileType, FollowSymlinks, Inode as ExtInode,
    InodeCreationOptions, InodeFlags, InodeMode, Path,
};

use super::io::{block_on_future, map_ext_error};

pub(crate) struct ExtInodeNode {
    pub(crate) filesystem: Ext4,
    pub(crate) path: String,
    pub(crate) name: String,
    pub(crate) kind: NodeKind,
    pub(crate) inode: SpinLock<ExtInode>,
    pub(crate) metadata: SpinLock<NodeMetadata>,
    pub(crate) symlink_target: Option<String>,
    pub(crate) io_lock: Mutex<()>,
    pub(crate) open_state: Mutex<ExtOpenState>,
    pub(crate) children: SpinLock<BTreeMap<String, NodeRef>>,
}

#[derive(Default)]
pub(crate) struct ExtOpenState {
    pub(crate) refs: usize,
    pub(crate) file: Option<ExtFile>,
}

impl ExtInodeNode {
    pub(crate) fn new_ref(
        filesystem: Ext4,
        path: String,
        name: String,
        kind: NodeKind,
        inode: ExtInode,
        metadata: NodeMetadata,
        symlink_target: Option<String>,
    ) -> NodeRef {
        Inode::new(Arc::new(Self {
            filesystem,
            path,
            name,
            kind,
            inode: SpinLock::new(inode),
            metadata: SpinLock::new(metadata),
            symlink_target,
            io_lock: Mutex::new(()),
            open_state: Mutex::new(ExtOpenState::default()),
            children: SpinLock::new(BTreeMap::new()),
        }))
    }

    pub(crate) fn lock_io(&self) -> MutexGuard<'_, ()> {
        self.io_lock.lock()
    }

    pub(crate) fn child_path(&self, name: &str) -> String {
        if self.path == "/" {
            alloc::format!("/{name}")
        } else {
            alloc::format!("{}/{}", self.path, name)
        }
    }

    pub(crate) fn parent_dir_and_name<'a>(
        &self,
        child_name: &'a str,
    ) -> FsResult<(Dir, DirEntryName<'a>)> {
        if self.kind != NodeKind::Directory {
            return Err(FsError::NotDirectory);
        }
        let parent_inode = self.inode.lock().clone();
        let dir = Dir::open_inode(&self.filesystem, parent_inode).map_err(map_ext_error)?;
        let name = DirEntryName::try_from(child_name).map_err(|_| FsError::InvalidInput)?;
        Ok((dir, name))
    }

    pub(crate) fn create_inode(
        &self,
        file_type: FileType,
        mode: InodeMode,
        flags: InodeFlags,
    ) -> FsResult<ExtInode> {
        block_on_future(self.filesystem.create_inode(InodeCreationOptions {
            file_type,
            mode,
            uid: 0,
            gid: 0,
            time: Duration::ZERO,
            flags,
        }))
        .map_err(map_ext_error)
    }

    pub(crate) fn open_file(&self) -> FsResult<ExtFile> {
        let inode = self.inode.lock().clone();
        ExtFile::open_inode(&self.filesystem, inode).map_err(map_ext_error)
    }

    pub(crate) fn with_open_file<T>(
        &self,
        f: impl FnOnce(&mut ExtFile) -> FsResult<T>,
    ) -> FsResult<T> {
        let mut state = self.open_state.lock();
        if state.refs == 0 {
            let mut file = self.open_file()?;
            let result = f(&mut file);
            self.store_inode(file.inode().clone());
            return result;
        }

        if state.file.is_none() {
            state.file = Some(self.open_file()?);
        }
        let file = state.file.as_mut().ok_or(FsError::InvalidInput)?;
        let result = f(file);
        self.store_inode(file.inode().clone());
        result
    }

    pub(crate) fn store_inode(&self, inode: ExtInode) {
        *self.inode.lock() = inode.clone();
        *self.metadata.lock() = ext_node_metadata(inode.index.get() as u64, inode.metadata());
    }

    pub(crate) fn lookup_cached_child(&self, name: &str) -> Option<NodeRef> {
        self.children.lock().get(name).cloned()
    }

    pub(crate) fn cache_child(&self, name: &str, node: &NodeRef) {
        self.children
            .lock()
            .insert(String::from(name), node.clone());
    }

    pub(crate) fn invalidate_child(&self, name: &str) {
        let _ = self.children.lock().remove(name);
    }
}

pub(crate) fn load_inode_node(filesystem: Ext4, path: &str, name: &str) -> FsResult<NodeRef> {
    let inode = block_on_future(filesystem.path_to_inode(
        Path::try_from(path).map_err(|_| FsError::InvalidInput)?,
        FollowSymlinks::ExcludeFinalComponent,
    ))
    .map_err(map_ext_error)?;
    load_inode_node_from_ext_inode(filesystem, inode, path, name)
}

pub(crate) fn load_inode_node_from_ext_inode(
    filesystem: Ext4,
    inode: ExtInode,
    path: &str,
    name: &str,
) -> FsResult<NodeRef> {
    let inode_number = inode.index.get() as u64;
    let metadata = inode.metadata();
    let file_type = metadata.file_type();
    let kind = if metadata.is_dir() {
        NodeKind::Directory
    } else if file_type.is_socket() {
        NodeKind::Socket
    } else if metadata.is_symlink() {
        NodeKind::Symlink
    } else {
        NodeKind::File
    };
    let symlink_target = (kind == NodeKind::Symlink)
        .then(|| block_on_future(inode.symlink_target(&filesystem)).map_err(map_ext_error))
        .transpose()?
        .map(|target| target.display().to_string());

    Ok(ExtInodeNode::new_ref(
        filesystem,
        String::from(path),
        String::from(name),
        kind,
        inode,
        ext_node_metadata(inode_number, metadata),
        symlink_target,
    ))
}

pub(crate) fn ext_node_metadata(inode: u64, metadata: ext4plus::prelude::Metadata) -> NodeMetadata {
    NodeMetadata {
        inode,
        mode: metadata.mode.bits() as u32,
        uid: metadata.uid,
        gid: metadata.gid,
        nlink: u32::from(metadata.links_count),
        size: metadata.size_in_bytes,
        block_size: 4096,
        blocks: metadata.size_in_bytes.div_ceil(512),
        atime: ext_timestamp(metadata.atime),
        mtime: ext_timestamp(metadata.mtime),
        ctime: ext_timestamp(metadata.ctime),
        btime: metadata.crtime.map(ext_timestamp).unwrap_or_default(),
        ..NodeMetadata::default()
    }
}

pub(crate) fn ext_timestamp(duration: Duration) -> NodeTimestamp {
    NodeTimestamp {
        secs: duration.as_secs().min(i64::MAX as u64) as i64,
        nanos: duration.subsec_nanos(),
    }
}

pub(crate) fn ext_mode(mode: u32, file_type: InodeMode) -> InodeMode {
    InodeMode::from_bits_retain((mode as u16 & 0o7777) | file_type.bits())
}

pub(crate) fn ext_block_on<F>(future: F) -> F::Output
where
    F: Future,
{
    block_on_future(future)
}
