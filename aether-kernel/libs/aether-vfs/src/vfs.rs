extern crate alloc;

use alloc::string::String;

use aether_frame::libs::spin::SpinLock;

use crate::{
    Dentry, DentryRef, DirectoryNode, FsError, FsResult, NodeKind, NodeRef, SuperBlock,
    SuperBlockRef, VfsPath, resolve_namespace_path, resolve_symlink_path, split_components,
};

pub struct Vfs {
    root_superblock: SpinLock<Option<SuperBlockRef>>,
    root: SpinLock<Option<DentryRef>>,
}

impl Default for Vfs {
    fn default() -> Self {
        Self::new()
    }
}

impl Vfs {
    pub const fn new() -> Self {
        Self {
            root_superblock: SpinLock::new(None),
            root: SpinLock::new(None),
        }
    }

    pub fn mount_root(&self, root: NodeRef) {
        let superblock = SuperBlock::shared("rootfs", root.metadata().device_id);
        root.bind_superblock(&superblock);
        let dentry = Dentry::new_root(root);
        superblock.set_root(dentry.clone());
        *self.root_superblock.lock_irqsave() = Some(superblock);
        *self.root.lock_irqsave() = Some(dentry);
    }

    pub fn replace_root(&self, root: NodeRef) -> Option<NodeRef> {
        let previous = self.root();
        self.mount_root(root);
        previous
    }

    pub fn root_superblock(&self) -> Option<SuperBlockRef> {
        self.root_superblock.lock_irqsave().clone()
    }

    pub fn root_dentry(&self) -> Option<DentryRef> {
        self.root.lock_irqsave().clone()
    }

    pub fn root(&self) -> Option<NodeRef> {
        self.root_dentry().map(|dentry| dentry.inode())
    }

    pub fn lookup_absolute(&self, path: &str) -> Option<NodeRef> {
        self.lookup_absolute_path(path).map(|entry| entry.node())
    }

    pub fn lookup_absolute_nofollow(&self, path: &str) -> Option<NodeRef> {
        self.lookup_absolute_path_nofollow(path)
            .map(|entry| entry.node())
    }

    pub fn lookup_from(&self, start: NodeRef, path: &str) -> Option<NodeRef> {
        self.lookup_impl(start, path, true)
    }

    pub fn lookup_from_nofollow(&self, start: NodeRef, path: &str) -> Option<NodeRef> {
        self.lookup_impl(start, path, false)
    }

    pub fn lookup_absolute_path(&self, path: &str) -> Option<VfsPath> {
        let root = self.root_dentry()?;
        self.lookup_path_impl(root, path, true)
    }

    pub fn lookup_absolute_path_nofollow(&self, path: &str) -> Option<VfsPath> {
        let root = self.root_dentry()?;
        self.lookup_path_impl(root, path, false)
    }

    pub fn lookup_path_from(&self, start: VfsPath, path: &str) -> Option<VfsPath> {
        if let Some(dentry) = start.dentry() {
            self.lookup_path_impl(dentry, path, true)
        } else {
            let node = self.lookup_from(start.node(), path)?;
            Some(VfsPath::new(
                resolve_namespace_path(start.path(), path),
                node,
            ))
        }
    }

    pub fn lookup_path_from_nofollow(&self, start: VfsPath, path: &str) -> Option<VfsPath> {
        if let Some(dentry) = start.dentry() {
            self.lookup_path_impl(dentry, path, false)
        } else {
            let node = self.lookup_from_nofollow(start.node(), path)?;
            Some(VfsPath::new(
                resolve_namespace_path(start.path(), path),
                node,
            ))
        }
    }

    fn lookup_impl(&self, start: NodeRef, path: &str, follow_final: bool) -> Option<NodeRef> {
        let mut current = if path.starts_with('/') {
            self.root()?
        } else {
            start
        };

        let components = split_components(path);
        for (index, component) in components.iter().enumerate() {
            let next = current.lookup(component)?;
            let is_final = index + 1 == components.len();
            current = if is_final && !follow_final {
                next
            } else {
                self.resolve_node(next)?
            };
        }

        if follow_final {
            self.resolve_node(current)
        } else {
            Some(current)
        }
    }

    fn lookup_path_impl(
        &self,
        start: DentryRef,
        path: &str,
        follow_final: bool,
    ) -> Option<VfsPath> {
        let absolute = if path.starts_with('/') {
            resolve_namespace_path("/", path)
        } else {
            resolve_namespace_path(start.absolute_path().as_str(), path)
        };
        let root = self.root_dentry()?;
        let mut current = root;
        let components = split_components(absolute.as_str());

        if components.is_empty() {
            return Some(VfsPath::from_dentry(String::from("/"), current));
        }

        for (index, component) in components.iter().enumerate() {
            let next = Dentry::lookup(&current, component)?;
            let is_final = index + 1 == components.len();
            current = if is_final && !follow_final {
                next
            } else {
                self.resolve_dentry(next)?
            };
        }

        if follow_final {
            current = self.resolve_dentry(current)?;
        }

        Some(VfsPath::from_dentry(absolute, current))
    }

    pub fn ensure_dir_from(&self, start: NodeRef, path: &str) -> FsResult<NodeRef> {
        let mut current = if path.starts_with('/') {
            self.root().ok_or(FsError::RootNotMounted)?
        } else {
            start
        };

        for component in split_components(path) {
            if let Some(existing) = current.lookup(&component) {
                if existing.kind() != NodeKind::Directory {
                    return Err(FsError::NotDirectory);
                }
                current = existing;
                continue;
            }

            let next = DirectoryNode::new(component.clone());
            current.insert_child(component, next.clone())?;
            current = next;
        }

        Ok(current)
    }

    pub fn install_at(&self, start: NodeRef, path: &str, node: NodeRef) -> FsResult<()> {
        let mut components = split_components(path);
        let name = components.pop().ok_or(FsError::InvalidInput)?;
        let parent = if components.is_empty() {
            start
        } else {
            self.ensure_dir_from(start, components.join("/").as_str())?
        };
        parent.insert_child(name, node)
    }

    fn resolve_dentry(&self, mut dentry: DentryRef) -> Option<DentryRef> {
        for _ in 0..40 {
            if dentry.inode().kind() != NodeKind::Symlink {
                return Some(dentry);
            }

            let target = String::from(dentry.inode().symlink_target()?);
            let parent = dentry.parent();
            let base = parent
                .as_ref()
                .map(|entry| entry.absolute_path())
                .unwrap_or_else(|| String::from("/"));
            let next_path = resolve_symlink_path(base.as_str(), target.as_str());
            dentry = self
                .lookup_absolute_path_nofollow(next_path.as_str())?
                .dentry()?;
        }
        None
    }

    fn resolve_node(&self, mut node: NodeRef) -> Option<NodeRef> {
        for _ in 0..40 {
            if node.kind() != NodeKind::Symlink {
                return Some(node);
            }
            let target = String::from(node.symlink_target()?);
            node = self.lookup_absolute(target.as_str())?;
        }
        None
    }
}
