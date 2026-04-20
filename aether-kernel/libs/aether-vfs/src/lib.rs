#![no_std]

extern crate alloc;

mod dentry;
mod epoll;
mod file;
mod inode;
mod node;
mod page_cache;
mod path;
mod superblock;
mod vfs;
mod wait;

pub use self::dentry::{Dentry, DentryRef};
pub use self::epoll::{
    EpollCtlOp, EpollData, EpollEvent, EpollEvents, EpollInstance, SharedEpollInstance,
    create_epoll_instance,
};
pub use self::file::{
    FileAdvice, FlockOperation, IoctlResponse, MmapCachePolicy, MmapKind, MmapRequest,
    MmapResponse, OpenFileDescription, OpenFlags, PollEvents, SharedOpenFile, VfsFile,
};
pub use self::inode::{Inode, InodeOperations, NodeRef};
pub use self::node::{
    CowMemoryFile, DirectoryEntry, DirectoryNode, FileNode, FileOperations, FsError, FsResult,
    MemfdOptions, MemoryFile, MutableMemoryFile, NodeKind, NodeMetadata, NodeTimestamp,
    SharedMemoryFile, SymlinkNode,
};
pub use self::page_cache::reclaim_page_cache;
pub use self::path::{
    VfsPath, display_path_from_root, is_within, leaf_name, normalize_absolute_path, parent_path,
    remap_mount_path, resolve_namespace_path, resolve_symlink_path, resolve_view_path,
    split_components,
};
pub use self::superblock::{SuperBlock, SuperBlockRef};
pub use self::vfs::Vfs;
pub use self::wait::{SharedWaitListener, WaitListener, WaitQueue};
