extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;

use aether_frame::libs::mutex::Mutex;
use aether_vfs::{NodeRef, VfsPath};

use crate::errno::{SysErr, SysResult};
use crate::rootfs::filesystem::MountedNode;
use crate::rootfs::path::normalize_absolute_path;

pub struct MountNamespace {
    root_stack: alloc::vec::Vec<MountedNode>,
    mounts: BTreeMap<String, alloc::vec::Vec<MountedNode>>,
}

impl MountNamespace {
    pub fn new(root: MountedNode) -> Self {
        Self {
            root_stack: alloc::vec![root],
            mounts: BTreeMap::new(),
        }
    }

    pub fn root_mount(&self) -> MountedNode {
        self.root_stack
            .last()
            .cloned()
            .expect("mount namespace root stack must not be empty")
    }

    pub fn root_node(&self) -> aether_vfs::NodeRef {
        self.root_mount().node()
    }

    pub fn lookup_mount(&self, path: &str) -> Option<MountedNode> {
        if path == "/" {
            Some(self.root_mount())
        } else {
            self.mounts
                .get(path)
                .and_then(|stack| stack.last())
                .cloned()
        }
    }

    pub fn covering_mount(&self, path: &str) -> MountedNode {
        let path = normalize_absolute_path(path);
        let mut selected = self.root_mount();
        let mut selected_len = 1usize;

        for (mount_path, stack) in &self.mounts {
            if !mount_path_matches(path.as_str(), mount_path.as_str()) {
                continue;
            }
            if mount_path.len() < selected_len {
                continue;
            }
            if let Some(mount) = stack.last() {
                selected = mount.clone();
                selected_len = mount_path.len();
            }
        }

        selected
    }

    pub fn mount(&mut self, target: &str, node: MountedNode) -> SysResult<()> {
        let target = normalize_absolute_path(target);
        if target == "/" {
            self.root_stack.push(node);
        } else {
            self.mounts.entry(target).or_default().push(node);
        }
        Ok(())
    }

    pub fn move_mount(&mut self, source: &str, target: &str) -> SysResult<()> {
        let source = normalize_absolute_path(source);
        let target = normalize_absolute_path(target);

        let node = if source == "/" {
            self.root_mount()
        } else {
            let stack = self.mounts.get_mut(source.as_str()).ok_or(SysErr::Inval)?;
            let node = stack.pop().ok_or(SysErr::Inval)?;
            if stack.is_empty() {
                self.mounts.remove(source.as_str());
            }
            node
        };

        let descendants = self.rewrite_descendant_mounts(source.as_str(), target.as_str());

        if target == "/" {
            self.root_stack.push(node);
        } else {
            self.mounts.entry(target).or_default().push(node);
        }
        for (path, stack) in descendants {
            self.mounts.insert(path, stack);
        }
        Ok(())
    }

    pub fn unmount(&mut self, target: &str) -> SysResult<()> {
        let target = normalize_absolute_path(target);
        if target == "/" {
            if self.root_stack.len() <= 1 {
                return Err(SysErr::Inval);
            }
            let _ = self.root_stack.pop();
            return Ok(());
        }

        let stack = self.mounts.get_mut(target.as_str()).ok_or(SysErr::Inval)?;
        let _ = stack.pop().ok_or(SysErr::Inval)?;
        if stack.is_empty() {
            self.mounts.remove(target.as_str());
        }
        Ok(())
    }
}

impl MountNamespace {
    fn rewrite_descendant_mounts(
        &mut self,
        source: &str,
        target: &str,
    ) -> BTreeMap<String, alloc::vec::Vec<MountedNode>> {
        if source == "/" {
            return BTreeMap::new();
        }

        let source_prefix = alloc::format!("{source}/");
        let mut moved = BTreeMap::new();
        let keys = self
            .mounts
            .keys()
            .filter(|path| path.starts_with(source_prefix.as_str()))
            .cloned()
            .collect::<alloc::vec::Vec<_>>();

        for old_path in keys {
            let Some(stack) = self.mounts.remove(old_path.as_str()) else {
                continue;
            };
            let suffix = old_path
                .strip_prefix(source)
                .expect("descendant mount path must begin with source");
            let new_path = if target == "/" {
                String::from(suffix)
            } else {
                alloc::format!("{target}{suffix}")
            };
            moved.insert(normalize_absolute_path(new_path.as_str()), stack);
        }

        moved
    }
}

fn mount_path_matches(path: &str, mount_path: &str) -> bool {
    path == mount_path
        || (path.starts_with(mount_path)
            && path.as_bytes().get(mount_path.len()).copied() == Some(b'/'))
}

pub type FsLocation = VfsPath;

#[derive(Clone)]
pub struct ProcessFsContext {
    namespace: Arc<Mutex<MountNamespace>>,
    root: FsLocation,
    cwd: FsLocation,
}

impl ProcessFsContext {
    pub fn new(namespace: Arc<Mutex<MountNamespace>>) -> Self {
        let root_node = namespace.lock().root_node();
        let root = FsLocation::new(String::from("/"), root_node);
        Self {
            namespace,
            root: root.clone(),
            cwd: root,
        }
    }

    pub fn namespace(&self) -> Arc<Mutex<MountNamespace>> {
        self.namespace.clone()
    }

    pub fn root_path(&self) -> &str {
        self.root.path()
    }

    pub fn cwd_path(&self) -> &str {
        self.cwd.path()
    }

    pub fn root_node(&self) -> NodeRef {
        self.root.node()
    }

    pub fn cwd_node(&self) -> NodeRef {
        self.cwd.node()
    }

    pub fn set_root_location(&mut self, location: FsLocation) {
        self.root = location;
    }

    pub fn set_cwd_location(&mut self, location: FsLocation) {
        self.cwd = location;
    }

    pub fn rebind_root_path(&mut self, path: String) {
        self.root.rebind_path(path);
    }

    pub fn rebind_cwd_path(&mut self, path: String) {
        self.cwd.rebind_path(path);
    }
}
