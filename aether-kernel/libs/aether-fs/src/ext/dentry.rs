extern crate alloc;

use aether_frame::libs::spin::PreemptDisabled;
use aether_vfs::{
    DirectoryEntry, FsError, FsResult, InodeOperations, NodeKind, NodeMetadata, NodeRef,
};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::time::Duration;
use ext4plus::prelude::{AsyncIterator, Dir, FileType, InodeFlags, InodeMode, PathBuf};

use super::inode::{ExtInodeNode, ext_mode, load_inode_node_from_ext_inode};
use super::io::{block_on_future, map_ext_error};

struct LockedNodePair<'a> {
    _first: aether_frame::libs::spin::SpinLockGuard<'a, (), PreemptDisabled>,
    _second: Option<aether_frame::libs::spin::SpinLockGuard<'a, (), PreemptDisabled>>,
}

fn lock_node_pair<'a>(left: &'a ExtInodeNode, right: &'a ExtInodeNode) -> LockedNodePair<'a> {
    let left_ptr = left as *const ExtInodeNode as usize;
    let right_ptr = right as *const ExtInodeNode as usize;
    if left_ptr <= right_ptr {
        let first = left.lock_io();
        let second = (!core::ptr::eq(left, right)).then(|| right.lock_io());
        LockedNodePair {
            _first: first,
            _second: second,
        }
    } else {
        let first = right.lock_io();
        let second = Some(left.lock_io());
        LockedNodePair {
            _first: first,
            _second: second,
        }
    }
}

impl InodeOperations for ExtInodeNode {
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> NodeKind {
        self.kind
    }

    fn lookup(&self, name: &str) -> Option<NodeRef> {
        if self.kind != NodeKind::Directory {
            return None;
        }

        if let Some(node) = self.lookup_cached_child(name) {
            return Some(node);
        }

        let inode = {
            let _guard = self.lock_io();
            let parent = Dir::open_inode(&self.filesystem, self.inode.lock().clone()).ok()?;
            block_on_future(parent.get_entry(ext4plus::prelude::DirEntryName::try_from(name).ok()?))
                .ok()?
        };
        let node = load_inode_node_from_ext_inode(
            self.filesystem.clone(),
            inode,
            self.child_path(name).as_str(),
            name,
        )
        .ok()?;
        self.cache_child(name, &node);
        Some(node)
    }

    fn entries(&self) -> Vec<DirectoryEntry> {
        if self.kind != NodeKind::Directory {
            return Vec::new();
        }

        let mut entries = Vec::new();
        let _guard = self.lock_io();
        let Ok(directory) = Dir::open_inode(&self.filesystem, self.inode.lock().clone()) else {
            return entries;
        };
        let Ok(mut reader) = directory.read_dir() else {
            return entries;
        };

        while let Some(entry) = block_on_future(reader.next()) {
            let Ok(entry) = entry else {
                break;
            };
            let name = entry
                .file_name()
                .as_str()
                .map(String::from)
                .unwrap_or_else(|_| entry.file_name().display().to_string());
            if name == "lost+found" {
                continue;
            }
            let kind = match entry.file_type() {
                Ok(file_type) if file_type.is_dir() => NodeKind::Directory,
                Ok(file_type) if file_type.is_socket() => NodeKind::Socket,
                Ok(file_type) if file_type.is_symlink() => NodeKind::Symlink,
                Ok(_) => NodeKind::File,
                Err(_) => continue,
            };
            entries.push(DirectoryEntry { name, kind });
        }

        entries
    }

    fn file_ops(&self) -> Option<&dyn aether_vfs::FileOperations> {
        matches!(self.kind, NodeKind::File | NodeKind::Socket).then_some(self)
    }

    fn create_file(&self, name: String, mode: u32) -> FsResult<NodeRef> {
        let (mut parent, entry_name) = self.parent_dir_and_name(name.as_str())?;
        let mut inode = self.create_inode(
            FileType::Regular,
            ext_mode(mode, InodeMode::S_IFREG),
            InodeFlags::empty(),
        )?;
        block_on_future(parent.link(entry_name, &mut inode)).map_err(map_ext_error)?;
        self.store_inode(parent.inode().clone());
        let node = load_inode_node_from_ext_inode(
            self.filesystem.clone(),
            inode,
            self.child_path(name.as_str()).as_str(),
            name.as_str(),
        )?;
        self.cache_child(name.as_str(), &node);
        Ok(node)
    }

    fn create_dir(&self, name: String, mode: u32) -> FsResult<NodeRef> {
        let (mut parent, entry_name) = self.parent_dir_and_name(name.as_str())?;
        let parent_inode = parent.inode().clone();
        let inode = self.create_inode(
            FileType::Directory,
            ext_mode(mode, InodeMode::S_IFDIR),
            InodeFlags::empty(),
        )?;
        let mut directory = block_on_future(Dir::init(
            self.filesystem.clone(),
            inode,
            parent_inode.index,
        ))
        .map_err(map_ext_error)?;
        block_on_future(parent.link(entry_name, directory.inode_mut())).map_err(map_ext_error)?;
        self.store_inode(parent.inode().clone());
        let node = load_inode_node_from_ext_inode(
            self.filesystem.clone(),
            directory.inode().clone(),
            self.child_path(name.as_str()).as_str(),
            name.as_str(),
        )?;
        self.cache_child(name.as_str(), &node);
        Ok(node)
    }

    fn create_symlink(&self, name: String, target: String, mode: u32) -> FsResult<NodeRef> {
        let (mut parent, entry_name) = self.parent_dir_and_name(name.as_str())?;
        let _guard = self.lock_io();
        let path_target = PathBuf::try_from(target.as_str()).map_err(|_| FsError::InvalidInput)?;
        let inode = block_on_future(self.filesystem.symlink(
            &mut parent,
            entry_name,
            path_target,
            0,
            0,
            Duration::ZERO,
        ))
        .map_err(map_ext_error)?;
        self.store_inode(parent.inode().clone());

        let node = load_inode_node_from_ext_inode(
            self.filesystem.clone(),
            inode,
            self.child_path(name.as_str()).as_str(),
            name.as_str(),
        )?;
        node.set_mode(mode)?;
        self.cache_child(name.as_str(), &node);
        Ok(node)
    }

    fn create_socket(&self, name: String, mode: u32) -> FsResult<NodeRef> {
        let (mut parent, entry_name) = self.parent_dir_and_name(name.as_str())?;
        let mut inode = self.create_inode(
            FileType::Socket,
            ext_mode(mode, InodeMode::S_IFSOCK),
            InodeFlags::empty(),
        )?;
        block_on_future(parent.link(entry_name, &mut inode)).map_err(map_ext_error)?;
        self.store_inode(parent.inode().clone());
        let node = load_inode_node_from_ext_inode(
            self.filesystem.clone(),
            inode,
            self.child_path(name.as_str()).as_str(),
            name.as_str(),
        )?;
        self.cache_child(name.as_str(), &node);
        Ok(node)
    }

    fn remove_child(&self, name: &str, remove_directory: bool) -> FsResult<()> {
        let (mut parent, entry_name) = self.parent_dir_and_name(name)?;
        let inode = {
            let _guard = self.lock_io();
            block_on_future(parent.get_entry(entry_name)).map_err(map_ext_error)?
        };
        let metadata = inode.metadata();
        if metadata.is_dir() != remove_directory {
            return Err(FsError::InvalidInput);
        }
        block_on_future(parent.unlink(entry_name, inode)).map_err(map_ext_error)?;
        self.store_inode(parent.inode().clone());
        self.invalidate_child(name);
        Ok(())
    }

    fn rename_child(
        &self,
        old_name: &str,
        new_parent: &NodeRef,
        new_name: String,
        replace: bool,
    ) -> FsResult<()> {
        let target = new_parent
            .operations()
            .as_any()
            .downcast_ref::<ExtInodeNode>()
            .ok_or(FsError::Unsupported)?;

        let mut source_parent =
            Dir::open_inode(&self.filesystem, self.inode.lock().clone()).map_err(map_ext_error)?;
        let mut target_parent = Dir::open_inode(&target.filesystem, target.inode.lock().clone())
            .map_err(map_ext_error)?;
        let source_name = ext4plus::prelude::DirEntryName::try_from(old_name)
            .map_err(|_| FsError::InvalidInput)?;
        let _guard = lock_node_pair(self, target);
        let mut inode =
            { block_on_future(source_parent.get_entry(source_name)).map_err(map_ext_error)? };
        if inode.metadata().is_dir() {
            // TODO: ext4plus does not currently expose a safe, Linux-like directory rename helper.
            // Directory rename needs '..' maintenance and ordered parent updates, so keep rejecting
            // it explicitly until the backend grows a native primitive.
            return Err(FsError::Unsupported);
        }

        let target_name_initial = ext4plus::prelude::DirEntryName::try_from(new_name.as_str())
            .map_err(|_| FsError::InvalidInput)?;

        let target_name = if replace {
            let existing = block_on_future(target_parent.get_entry(target_name_initial));
            if let Ok(existing) = existing {
                block_on_future(target_parent.unlink(target_name_initial, existing))
                    .map_err(map_ext_error)?;
                target_name_initial
            } else {
                target_name_initial
            }
        } else {
            target_name_initial
        };

        block_on_future(target_parent.link(target_name, &mut inode)).map_err(map_ext_error)?;
        block_on_future(source_parent.unlink(source_name, inode)).map_err(map_ext_error)?;
        self.store_inode(source_parent.inode().clone());
        target.store_inode(target_parent.inode().clone());
        self.invalidate_child(old_name);
        target.invalidate_child(new_name.as_str());
        Ok(())
    }

    fn link_child(&self, name: String, existing: &NodeRef) -> FsResult<()> {
        let target = existing
            .operations()
            .as_any()
            .downcast_ref::<ExtInodeNode>()
            .ok_or(FsError::Unsupported)?;
        if target.kind == NodeKind::Directory {
            return Err(FsError::Unsupported);
        }

        let (mut parent, entry_name) = self.parent_dir_and_name(name.as_str())?;
        let _guard = lock_node_pair(self, target);
        let mut inode = target.inode.lock().clone();
        block_on_future(parent.link(entry_name, &mut inode)).map_err(map_ext_error)?;
        self.store_inode(parent.inode().clone());
        target.store_inode(inode);
        self.invalidate_child(name.as_str());
        Ok(())
    }

    fn symlink_target(&self) -> Option<&str> {
        self.symlink_target.as_deref()
    }

    fn mode(&self) -> Option<u32> {
        Some(self.metadata.lock().mode)
    }

    fn set_mode(&self, mode: u32) -> FsResult<()> {
        let _guard = self.lock_io();
        let mut inode = self.inode.lock().clone();
        let file_type = match self.kind {
            NodeKind::Directory => InodeMode::S_IFDIR,
            NodeKind::File => InodeMode::S_IFREG,
            NodeKind::Socket => InodeMode::S_IFSOCK,
            NodeKind::Symlink => InodeMode::S_IFLNK,
            NodeKind::BlockDevice => InodeMode::S_IFBLK,
            NodeKind::CharDevice => InodeMode::S_IFCHR,
            NodeKind::Fifo => InodeMode::S_IFIFO,
        };
        inode
            .set_mode(ext_mode(mode, file_type))
            .map_err(map_ext_error)?;
        block_on_future(inode.write(&self.filesystem)).map_err(map_ext_error)?;
        self.store_inode(inode);
        Ok(())
    }

    fn set_owner(&self, uid: u32, gid: u32) -> FsResult<()> {
        let _guard = self.lock_io();
        let mut inode = self.inode.lock().clone();
        inode.set_uid(uid);
        inode.set_gid(gid);
        block_on_future(inode.write(&self.filesystem)).map_err(map_ext_error)?;
        self.store_inode(inode);
        Ok(())
    }

    fn metadata(&self) -> NodeMetadata {
        *self.metadata.lock()
    }
}
