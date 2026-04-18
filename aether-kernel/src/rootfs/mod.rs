extern crate alloc;

mod filesystem;
mod namespace;
mod path;

use alloc::string::String;
use alloc::string::ToString;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

use aether_device::DeviceNamespace;
use aether_frame::boot;
use aether_frame::libs::mutex::Mutex;
use aether_frame::libs::spin::SpinLock;
use aether_initramfs::{InitramfsError, load_initramfs};
use aether_tmpfs as tmpfs;
use aether_vfs::{DirectoryNode, FsError, NodeKind, NodeRef, Vfs};

use crate::errno::{SysErr, SysResult};
use crate::fs::{FileSystemIdentity, LinuxStatFs};
use crate::kernfs::KernelResourceRegistry;
use crate::rootfs::filesystem::KernelFileSystem;

use self::filesystem::{
    BindFs, ExtFileSystem, FileSystemRegistry, MountRequest, MountedNode, PROC_SUPER_MAGIC,
    RAMFS_MAGIC, SYSFS_MAGIC, StaticDirectoryFs, TMPFS_MAGIC, TmpFs,
};
pub use self::namespace::{FsLocation, MountNamespace, ProcessFsContext};
use self::path::{
    display_path_from_root, is_within, leaf_name, normalize_absolute_path, parent_path,
    remap_mount_path, resolve_namespace_path, resolve_symlink_path, resolve_view_path,
    split_components,
};

#[derive(Debug)]
pub enum RootfsError {
    FileSystem(FsError),
    Initramfs(InitramfsError),
}

impl From<FsError> for RootfsError {
    fn from(value: FsError) -> Self {
        Self::FileSystem(value)
    }
}

impl From<InitramfsError> for RootfsError {
    fn from(value: InitramfsError) -> Self {
        Self::Initramfs(value)
    }
}

pub struct RootfsManager {
    vfs: Vfs,
    filesystems: FileSystemRegistry,
    bind_fs: Arc<BindFs>,
    next_mount_device: SpinLock<u64>,
    boot_root: MountedNode,
    dev_root: MountedNode,
    device_namespace: DeviceNamespace,
    resource_registry: KernelResourceRegistry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecFormat {
    Flat,
    Elf,
}

pub struct ExecPlan {
    pub requested_path: String,
    pub exec_path: String,
    pub argv: Vec<String>,
    pub envp: Vec<String>,
    pub node: NodeRef,
    pub format: ExecFormat,
}

struct NamespaceLookup {
    node: NodeRef,
    path: String,
}

impl RootfsManager {
    pub fn new() -> Result<Self, RootfsError> {
        let vfs = Vfs::new();
        let boot_root_node = if let Some(initrd) = boot::initrd_bytes() {
            log::info!("rootfs: loading initramfs image ({} bytes)", initrd.len());
            load_initramfs(initrd, &vfs)?
        } else {
            log::warn!("rootfs: no initramfs module found, using empty root");
            tmpfs::directory("/")
        };
        vfs.mount_root(boot_root_node.clone());

        let dev_root_node: NodeRef = DirectoryNode::new("devtmpfs");
        let proc_root_node: NodeRef = DirectoryNode::new("proc");
        let sys_root_node: NodeRef = DirectoryNode::new("sysfs");
        let device_namespace = DeviceNamespace::new(dev_root_node.clone());

        let boot_fs = Arc::new(StaticDirectoryFs::new(
            "rootfs",
            RAMFS_MAGIC,
            boot_root_node,
        ));
        let dev_fs = Arc::new(StaticDirectoryFs::new(
            "devtmpfs",
            TMPFS_MAGIC,
            dev_root_node,
        ));
        let proc_fs = Arc::new(StaticDirectoryFs::new(
            "proc",
            PROC_SUPER_MAGIC,
            proc_root_node,
        ));
        let sys_fs = Arc::new(StaticDirectoryFs::new("sysfs", SYSFS_MAGIC, sys_root_node));
        let tmp_fs = Arc::new(TmpFs);
        let bind_fs = Arc::new(BindFs);
        let mut next_mount_device = 1u64;
        let mut alloc_mount_device = || {
            let device_id = next_mount_device;
            next_mount_device = next_mount_device.saturating_add(1);
            device_id
        };

        let mut filesystems = FileSystemRegistry::new();
        filesystems.register(boot_fs.clone());
        filesystems.register(dev_fs.clone());
        filesystems.register(proc_fs.clone());
        filesystems.register(sys_fs.clone());
        filesystems.register(tmp_fs);
        filesystems.register(Arc::new(ExtFileSystem::new("ext")));
        filesystems.register(Arc::new(ExtFileSystem::new("ext2")));
        filesystems.register(Arc::new(ExtFileSystem::new("ext3")));
        filesystems.register(Arc::new(ExtFileSystem::new("ext4")));

        let boot_root_mount = boot_fs
            .mount(&MountRequest {
                target_name: String::from("/"),
                source: None,
            })
            .expect("static root mount");
        let dev_root_mount = dev_fs
            .mount(&MountRequest {
                target_name: String::from("devtmpfs"),
                source: None,
            })
            .expect("static devtmpfs mount");
        let proc_root_mount = proc_fs
            .mount(&MountRequest {
                target_name: String::from("proc"),
                source: None,
            })
            .expect("static proc mount");
        let sys_root_mount = sys_fs
            .mount(&MountRequest {
                target_name: String::from("sysfs"),
                source: None,
            })
            .expect("static sysfs mount");

        let boot_root = MountedNode::new(
            boot_root_mount.root,
            boot_fs,
            alloc_mount_device(),
            boot_root_mount.statfs,
        );
        let dev_root = MountedNode::new(
            dev_root_mount.root,
            dev_fs,
            alloc_mount_device(),
            dev_root_mount.statfs,
        );
        let proc_root_node = proc_root_mount.root.clone();
        let sys_root_node = sys_root_mount.root.clone();
        let resource_registry = KernelResourceRegistry::new(
            proc_root_node,
            sys_root_node,
            boot::info().command_line,
            filesystems.types().as_slice(),
        )?;
        let _proc_root = MountedNode::new(
            proc_root_mount.root,
            proc_fs,
            alloc_mount_device(),
            proc_root_mount.statfs,
        );
        let _sys_root = MountedNode::new(
            sys_root_mount.root,
            sys_fs,
            alloc_mount_device(),
            sys_root_mount.statfs,
        );

        Ok(Self {
            vfs,
            filesystems,
            bind_fs: bind_fs.clone(),
            next_mount_device: SpinLock::new(next_mount_device),
            boot_root,
            dev_root,
            device_namespace,
            resource_registry,
        })
    }

    pub fn vfs(&self) -> &Vfs {
        &self.vfs
    }

    pub fn device_namespace(&self) -> &DeviceNamespace {
        &self.device_namespace
    }

    pub fn device_filesystem_identity(&self) -> FileSystemIdentity {
        self.mount_identity(&self.dev_root)
    }

    pub fn register_device(&self, device: Arc<dyn aether_device::KernelDevice>) -> SysResult<()> {
        self.resource_registry
            .register_device(&self.vfs, &self.device_namespace, device)
            .map_err(SysErr::from)
    }

    pub fn register_pci_bus(&self) -> SysResult<()> {
        self.resource_registry
            .register_pci_bus()
            .map_err(SysErr::from)
    }

    pub fn initial_fs_context(&self) -> ProcessFsContext {
        ProcessFsContext::new(Arc::new(Mutex::new(MountNamespace::new(
            self.boot_root.clone(),
        ))))
    }

    pub fn lookup_in(
        &self,
        fs: &ProcessFsContext,
        path: &str,
        follow_final: bool,
    ) -> SysResult<NodeRef> {
        self.lookup_in_with_identity(fs, path, follow_final)
            .map(|(node, _)| node)
    }

    pub fn lookup_in_with_identity(
        &self,
        fs: &ProcessFsContext,
        path: &str,
        follow_final: bool,
    ) -> SysResult<(NodeRef, FileSystemIdentity)> {
        let namespace_path = resolve_view_path(fs.root_path(), fs.cwd_path(), path);
        let namespace = {
            let namespace = fs.namespace();
            namespace.lock().clone()
        };
        if let Some(result) = crate::procfs::lookup_virtual(namespace_path.as_str()) {
            let mount = namespace.covering_mount(namespace_path.as_str());
            return result.map(|node| (node, self.mount_identity(&mount)));
        }
        let lookup =
            self.lookup_namespace_entry(&namespace, namespace_path.as_str(), follow_final, 0)?;
        let mount = namespace.covering_mount(lookup.path.as_str());
        Ok((lookup.node, self.mount_identity(&mount)))
    }

    pub fn statfs_in(&self, fs: &ProcessFsContext, path: &str) -> SysResult<LinuxStatFs> {
        let namespace_path = resolve_view_path(fs.root_path(), fs.cwd_path(), path);
        let namespace = {
            let namespace = fs.namespace();
            namespace.lock().clone()
        };
        let lookup = self.lookup_namespace_entry(&namespace, namespace_path.as_str(), true, 0)?;
        Ok(namespace.covering_mount(lookup.path.as_str()).statfs())
    }

    pub fn mkdir_in(&self, fs: &ProcessFsContext, path: &str, mode: u32) -> SysResult<u64> {
        let namespace_path = resolve_view_path(fs.root_path(), fs.cwd_path(), path);
        let node = self.create_dir_namespace(fs, namespace_path.as_str(), mode)?;
        let namespace = fs.namespace();
        let namespace = namespace.lock();
        let parent =
            self.lookup_namespace_path(&namespace, parent_path(namespace_path.as_str()), true, 0)?;
        crate::fs::notify_create(&parent, &node, leaf_name(namespace_path.as_str()));
        Ok(0)
    }

    pub fn create_file_in(
        &self,
        fs: &ProcessFsContext,
        path: &str,
        mode: u32,
    ) -> SysResult<(NodeRef, FileSystemIdentity)> {
        let namespace_path = resolve_view_path(fs.root_path(), fs.cwd_path(), path);
        let node = self.create_file_namespace(fs, namespace_path.as_str(), mode)?;
        let namespace = fs.namespace();
        let namespace = namespace.lock();
        let parent =
            self.lookup_namespace_path(&namespace, parent_path(namespace_path.as_str()), true, 0)?;
        crate::fs::notify_create(&parent, &node, leaf_name(namespace_path.as_str()));
        let mount = namespace.covering_mount(namespace_path.as_str());
        Ok((node, self.mount_identity(&mount)))
    }

    pub fn create_symlink_in(
        &self,
        fs: &ProcessFsContext,
        path: &str,
        target: &str,
    ) -> SysResult<u64> {
        let namespace_path = resolve_view_path(fs.root_path(), fs.cwd_path(), path);
        let node = self.create_symlink_namespace(fs, namespace_path.as_str(), target, 0o120777)?;
        let namespace = fs.namespace();
        let namespace = namespace.lock();
        let parent =
            self.lookup_namespace_path(&namespace, parent_path(namespace_path.as_str()), true, 0)?;
        crate::fs::notify_create(&parent, &node, leaf_name(namespace_path.as_str()));
        Ok(0)
    }

    pub fn bind_socket_in(&self, fs: &ProcessFsContext, path: &str, mode: u32) -> SysResult<u64> {
        let namespace_path = resolve_view_path(fs.root_path(), fs.cwd_path(), path);
        let node = self.create_socket_namespace(fs, namespace_path.as_str(), mode)?;
        let namespace = fs.namespace();
        let namespace = namespace.lock();
        let parent =
            self.lookup_namespace_path(&namespace, parent_path(namespace_path.as_str()), true, 0)?;
        crate::fs::notify_create(&parent, &node, leaf_name(namespace_path.as_str()));
        Ok(0)
    }

    pub fn getcwd_in(&self, fs: &ProcessFsContext) -> String {
        display_path_from_root(fs.root_path(), fs.cwd_path())
    }

    pub fn unlink_in(&self, fs: &ProcessFsContext, path: &str, flags: u64) -> SysResult<u64> {
        const AT_REMOVEDIR: u64 = 0x200;

        let namespace_path = resolve_view_path(fs.root_path(), fs.cwd_path(), path);
        let normalized = normalize_absolute_path(namespace_path.as_str());
        if normalized == "/" {
            return Err(SysErr::Inval);
        }

        let namespace = fs.namespace();
        let namespace = namespace.lock();
        if namespace.lookup_mount(normalized.as_str()).is_some() {
            return Err(SysErr::Inval);
        }

        let parent =
            self.lookup_namespace_path(&namespace, parent_path(normalized.as_str()), true, 0)?;
        if parent.kind() != NodeKind::Directory {
            return Err(SysErr::NotDir);
        }

        let name = leaf_name(normalized.as_str());
        if name.is_empty() {
            return Err(SysErr::Inval);
        }

        let victim = parent.lookup(name).ok_or(SysErr::NoEnt)?;
        if victim.kind() == NodeKind::Directory {
            if (flags & AT_REMOVEDIR) == 0 {
                return Err(SysErr::IsDir);
            }
            if !victim.entries().is_empty() {
                return Err(SysErr::NotEmpty);
            }
        } else if (flags & AT_REMOVEDIR) != 0 {
            return Err(SysErr::NotDir);
        }

        parent
            .remove_child(name, (flags & AT_REMOVEDIR) != 0)
            .map_err(SysErr::from)?;
        crate::fs::notify_delete(&parent, &victim, name);
        Ok(0)
    }

    pub fn link_in(
        &self,
        fs: &ProcessFsContext,
        old_path: &str,
        new_path: &str,
        flags: u64,
    ) -> SysResult<u64> {
        const AT_SYMLINK_FOLLOW: u64 = 0x400;
        const AT_EMPTY_PATH: u64 = 0x1000;

        if (flags & !(AT_SYMLINK_FOLLOW | AT_EMPTY_PATH)) != 0 {
            return Err(SysErr::Inval);
        }
        if old_path.is_empty() || new_path.is_empty() {
            return Err(SysErr::NoEnt);
        }
        if (flags & AT_EMPTY_PATH) != 0 {
            // TODO: Linux linkat(AT_EMPTY_PATH) links an already-open file descriptor target.
            // That needs fd-backed source resolution, so keep rejecting it explicitly for now.
            return Err(SysErr::Inval);
        }

        let old_namespace_path = resolve_view_path(fs.root_path(), fs.cwd_path(), old_path);
        let new_namespace_path = resolve_view_path(fs.root_path(), fs.cwd_path(), new_path);
        let old_normalized = normalize_absolute_path(old_namespace_path.as_str());
        let new_normalized = normalize_absolute_path(new_namespace_path.as_str());

        if old_normalized == "/" || new_normalized == "/" {
            return Err(SysErr::Perm);
        }

        let namespace = fs.namespace();
        let namespace = namespace.lock();

        let follow_source = (flags & AT_SYMLINK_FOLLOW) != 0;
        let source_lookup =
            self.lookup_namespace_entry(&namespace, old_normalized.as_str(), follow_source, 0)?;
        let source = source_lookup.node;
        if source.kind() == NodeKind::Directory {
            return Err(SysErr::Perm);
        }

        let source_mount = namespace.covering_mount(source_lookup.path.as_str());
        let target_mount = namespace.covering_mount(new_normalized.as_str());
        if source_mount.device_id() != target_mount.device_id() {
            return Err(SysErr::XDev);
        }

        if namespace.lookup_mount(new_normalized.as_str()).is_some() {
            return Err(SysErr::Exists);
        }

        let parent =
            self.lookup_namespace_path(&namespace, parent_path(new_normalized.as_str()), true, 0)?;
        if parent.kind() != NodeKind::Directory {
            return Err(SysErr::NotDir);
        }

        let name = leaf_name(new_normalized.as_str());
        if name.is_empty() {
            return Err(SysErr::NoEnt);
        }
        if parent.lookup(name).is_some() {
            return Err(SysErr::Exists);
        }

        parent
            .link_child(String::from(name), &source)
            .map_err(|error| match error {
                aether_vfs::FsError::Unsupported if source.kind() == NodeKind::Directory => {
                    SysErr::Perm
                }
                other => SysErr::from(other),
            })?;
        crate::fs::notify_create(&parent, &source, name);
        Ok(0)
    }

    pub fn rename_in(
        &self,
        fs: &ProcessFsContext,
        old_path: &str,
        new_path: &str,
    ) -> SysResult<u64> {
        if old_path.is_empty() || new_path.is_empty() {
            return Err(SysErr::NoEnt);
        }

        let old_namespace_path = resolve_view_path(fs.root_path(), fs.cwd_path(), old_path);
        let new_namespace_path = resolve_view_path(fs.root_path(), fs.cwd_path(), new_path);
        let old_normalized = normalize_absolute_path(old_namespace_path.as_str());
        let new_normalized = normalize_absolute_path(new_namespace_path.as_str());

        if old_normalized == new_normalized {
            return Ok(0);
        }
        if old_normalized == "/" || new_normalized == "/" {
            return Err(SysErr::Busy);
        }
        if new_normalized.starts_with(old_normalized.as_str())
            && new_normalized.as_bytes().get(old_normalized.len()).copied() == Some(b'/')
        {
            let namespace = fs.namespace();
            let namespace = namespace.lock();
            let source =
                self.lookup_namespace_path(&namespace, old_normalized.as_str(), false, 0)?;
            if source.kind() == NodeKind::Directory {
                return Err(SysErr::Inval);
            }
        }

        let namespace = fs.namespace();
        let namespace = namespace.lock();
        if namespace.lookup_mount(old_normalized.as_str()).is_some()
            || namespace.lookup_mount(new_normalized.as_str()).is_some()
        {
            return Err(SysErr::Busy);
        }

        if namespace
            .covering_mount(old_normalized.as_str())
            .device_id()
            != namespace
                .covering_mount(new_normalized.as_str())
                .device_id()
        {
            return Err(SysErr::XDev);
        }

        let source_parent =
            self.lookup_namespace_path(&namespace, parent_path(old_normalized.as_str()), true, 0)?;
        let target_parent =
            self.lookup_namespace_path(&namespace, parent_path(new_normalized.as_str()), true, 0)?;
        let source_name = leaf_name(old_normalized.as_str());
        let target_name = leaf_name(new_normalized.as_str());
        if source_name.is_empty() || target_name.is_empty() {
            return Err(SysErr::Inval);
        }

        let source = source_parent.lookup(source_name).ok_or(SysErr::NoEnt)?;
        let existing_target = target_parent.lookup(target_name);
        if let Some(ref target) = existing_target {
            if Arc::ptr_eq(&source, target) {
                return Ok(0);
            }
            match (source.kind(), target.kind()) {
                (NodeKind::Directory, NodeKind::Directory) if !target.entries().is_empty() => {
                    return Err(SysErr::NotEmpty);
                }
                (NodeKind::Directory, _) => return Err(SysErr::NotDir),
                (_, NodeKind::Directory) => return Err(SysErr::IsDir),
                _ => {}
            }
        }

        source_parent
            .rename_child(
                source_name,
                &target_parent,
                String::from(target_name),
                existing_target.is_some(),
            )
            .map_err(SysErr::from)?;
        crate::fs::notify_move(
            &source_parent,
            &target_parent,
            &source,
            source_name,
            target_name,
        );
        Ok(0)
    }

    pub fn chdir_in(&self, fs: &mut ProcessFsContext, path: &str) -> SysResult<u64> {
        let namespace_path = resolve_view_path(fs.root_path(), fs.cwd_path(), path);
        let namespace = {
            let namespace = fs.namespace();
            namespace.lock().clone()
        };
        let lookup = self.lookup_namespace_entry(&namespace, namespace_path.as_str(), true, 0)?;
        if lookup.node.kind() != NodeKind::Directory {
            return Err(SysErr::NotDir);
        }
        fs.set_cwd_location(FsLocation::new(lookup.path, lookup.node));
        Ok(0)
    }

    pub fn chroot_in(&self, fs: &mut ProcessFsContext, path: &str) -> SysResult<u64> {
        let namespace_path = resolve_view_path(fs.root_path(), fs.cwd_path(), path);
        let namespace = {
            let namespace = fs.namespace();
            namespace.lock().clone()
        };
        let lookup = self.lookup_namespace_entry(&namespace, namespace_path.as_str(), true, 0)?;
        if lookup.node.kind() != NodeKind::Directory {
            return Err(SysErr::NotDir);
        }
        fs.set_root_location(FsLocation::new(lookup.path.clone(), lookup.node));
        if !is_within(fs.root_path(), fs.cwd_path()) {
            fs.set_cwd_location(FsLocation::new(lookup.path, fs.root_node()));
        }
        Ok(0)
    }

    pub fn mount_in(
        &self,
        fs: &mut ProcessFsContext,
        source: Option<&str>,
        target: &str,
        fstype: Option<&str>,
        flags: u64,
    ) -> SysResult<u64> {
        const MS_BIND: u64 = 4096;
        const MS_MOVE: u64 = 8192;

        let target_path = resolve_view_path(fs.root_path(), fs.cwd_path(), target);

        if (flags & MS_MOVE) != 0 {
            let source = source.ok_or(SysErr::Inval)?;
            let source_path = resolve_view_path(fs.root_path(), fs.cwd_path(), source);
            let target_node = self.lookup_in(fs, target, false)?;
            let source_mount = {
                let namespace = fs.namespace();
                namespace.lock().lookup_mount(source_path.as_str())
            }
            .ok_or(SysErr::Inval)?;
            if target_node.kind() != NodeKind::Directory
                && source_mount.node().kind() == NodeKind::Directory
            {
                return Err(SysErr::NotDir);
            }
            let namespace = fs.namespace();
            namespace
                .lock()
                .move_mount(source_path.as_str(), target_path.as_str())?;
            self.rebind_fs_context_after_move(fs, source_path.as_str(), target_path.as_str());
            return Ok(0);
        }

        let target_node = self.lookup_in(fs, target, false)?;

        let mounted = if (flags & MS_BIND) != 0 {
            let source = source.ok_or(SysErr::Inval)?;
            self.lookup_mount_source(fs, source)?
        } else {
            self.filesystems.mount_with_device(
                fstype.ok_or(SysErr::Inval)?,
                &MountRequest {
                    target_name: leaf_name(target_path.as_str()).to_string(),
                    source: source.and_then(|path| self.lookup_in(fs, path, true).ok()),
                },
                self.allocate_mount_device_id(),
            )?
        };

        if target_node.kind() != NodeKind::Directory && mounted.node().kind() == NodeKind::Directory
        {
            return Err(SysErr::NotDir);
        }

        let namespace = fs.namespace();
        namespace
            .lock()
            .mount(target_path.as_str(), mounted)
            .map(|_| 0)
    }

    pub fn umount_in(&self, fs: &ProcessFsContext, target: &str, _flags: u64) -> SysResult<u64> {
        let target_path = resolve_view_path(fs.root_path(), fs.cwd_path(), target);
        let namespace = fs.namespace();
        namespace.lock().unmount(target_path.as_str()).map(|_| 0)
    }

    pub fn pivot_root_in(
        &self,
        fs: &mut ProcessFsContext,
        new_root: &str,
        put_old: &str,
    ) -> SysResult<u64> {
        let new_root_path = resolve_view_path(fs.root_path(), fs.cwd_path(), new_root);
        let put_old_path = resolve_view_path(fs.root_path(), fs.cwd_path(), put_old);
        if !is_within(new_root_path.as_str(), put_old_path.as_str())
            || put_old_path == new_root_path
        {
            return Err(SysErr::Inval);
        }

        let namespace = fs.namespace();
        let mut namespace = namespace.lock();
        let new_root_node =
            self.lookup_namespace_path(&namespace, new_root_path.as_str(), true, 0)?;
        if new_root_node.kind() != NodeKind::Directory {
            return Err(SysErr::NotDir);
        }

        let old_root = namespace.root_mount();
        let _ = self.ensure_dir_namespace_locked(&namespace, put_old_path.as_str())?;
        namespace.mount(put_old_path.as_str(), old_root)?;
        let new_root_mount = namespace
            .lookup_mount(new_root_path.as_str())
            .unwrap_or_else(|| {
                MountedNode::new(
                    new_root_node,
                    self.bind_fs.clone(),
                    self.allocate_mount_device_id(),
                    namespace.covering_mount(new_root_path.as_str()).statfs(),
                )
            });
        namespace.mount("/", new_root_mount)?;
        drop(namespace);
        let root_node = fs.namespace().lock().root_node();
        let root_location = FsLocation::new(String::from("/"), root_node);
        fs.set_root_location(root_location.clone());
        fs.set_cwd_location(root_location);
        Ok(0)
    }

    pub fn prepare_exec(
        &self,
        fs: &ProcessFsContext,
        path: &str,
        argv: Vec<String>,
        envp: Vec<String>,
    ) -> SysResult<ExecPlan> {
        self.prepare_exec_inner(fs, path, argv, envp, 0)
    }

    fn prepare_exec_inner(
        &self,
        fs: &ProcessFsContext,
        path: &str,
        argv: Vec<String>,
        envp: Vec<String>,
        depth: usize,
    ) -> SysResult<ExecPlan> {
        if depth > 4 {
            return Err(SysErr::Inval);
        }

        let exec_path = self.resolve_exec_path(fs, path)?;
        let node = self.lookup_in(fs, exec_path.as_str(), true)?;
        if node.kind() != NodeKind::File {
            return Err(SysErr::Inval);
        }

        let prefix = self.read_prefix(&node, 256)?;
        if let Some((interpreter, argument)) = parse_shebang(&prefix) {
            let mut next_argv = Vec::with_capacity(argv.len() + 3);
            next_argv.push(interpreter.to_string());
            if let Some(argument) = argument {
                next_argv.push(argument.to_string());
            }
            next_argv.push(path.to_string());
            if argv.len() > 1 {
                next_argv.extend_from_slice(&argv[1..]);
            }

            return self.prepare_exec_inner(fs, interpreter, next_argv, envp, depth + 1);
        }

        let format = if prefix.starts_with(b"\x7fELF") {
            ExecFormat::Elf
        } else {
            ExecFormat::Flat
        };

        Ok(ExecPlan {
            requested_path: path.to_string(),
            exec_path,
            argv,
            envp,
            node,
            format,
        })
    }

    fn resolve_exec_path(&self, fs: &ProcessFsContext, path: &str) -> SysResult<String> {
        self.resolve_exec_path_inner(fs, path, 0)
    }

    fn resolve_exec_path_inner(
        &self,
        fs: &ProcessFsContext,
        path: &str,
        depth: usize,
    ) -> SysResult<String> {
        if depth > 8 {
            return Err(SysErr::Loop);
        }

        let namespace_path = resolve_view_path(fs.root_path(), fs.cwd_path(), path);
        let namespace = {
            let namespace = fs.namespace();
            namespace.lock().clone()
        };
        let node = self.lookup_namespace_path(&namespace, namespace_path.as_str(), false, 0)?;
        if node.kind() != NodeKind::Symlink {
            return Ok(namespace_path);
        }

        let target = node.symlink_target().ok_or(SysErr::Inval)?;
        let next = resolve_symlink_path(parent_path(namespace_path.as_str()), target);
        self.resolve_exec_path_inner(fs, next.as_str(), depth + 1)
    }

    fn read_prefix(&self, node: &NodeRef, len: usize) -> SysResult<Vec<u8>> {
        node.open();
        let mut bytes = vec![0; len];
        let read = node.read(0, &mut bytes).map_err(SysErr::from);
        node.release();
        let read = read?;
        bytes.truncate(read);
        Ok(bytes)
    }

    fn rebind_fs_context_after_move(&self, fs: &mut ProcessFsContext, source: &str, target: &str) {
        fs.rebind_root_path(remap_mount_path(fs.root_path(), source, target));
        fs.rebind_cwd_path(remap_mount_path(fs.cwd_path(), source, target));
    }

    fn lookup_namespace_path(
        &self,
        namespace: &MountNamespace,
        path: &str,
        follow_final: bool,
        depth: usize,
    ) -> SysResult<NodeRef> {
        self.lookup_namespace_entry(namespace, path, follow_final, depth)
            .map(|lookup| lookup.node)
    }

    fn lookup_namespace_entry(
        &self,
        namespace: &MountNamespace,
        path: &str,
        follow_final: bool,
        depth: usize,
    ) -> SysResult<NamespaceLookup> {
        if depth > 8 {
            return Err(SysErr::Loop);
        }

        let normalized = normalize_absolute_path(path);
        if normalized == "/" {
            return Ok(NamespaceLookup {
                node: namespace.root_node(),
                path: String::from("/"),
            });
        }

        let mut current = namespace.root_node();
        let mut current_path = String::from("/");
        let components = split_components(normalized.as_str());

        for (index, component) in components.iter().enumerate() {
            let next_path = resolve_namespace_path(current_path.as_str(), component.as_str());
            let next = if let Some(mount) = namespace.lookup_mount(next_path.as_str()) {
                mount.node()
            } else {
                match current.lookup(component) {
                    Some(node) => node,
                    None => return Err(SysErr::NoEnt),
                }
            };
            let is_final = index + 1 == components.len();

            if next.kind() == NodeKind::Symlink && (!is_final || follow_final) {
                let target = next.symlink_target().ok_or(SysErr::Inval)?;
                let mut redirected = resolve_symlink_path(current_path.as_str(), target);
                if !is_final {
                    for tail in &components[index + 1..] {
                        redirected = resolve_namespace_path(redirected.as_str(), tail.as_str());
                    }
                }
                return self.lookup_namespace_entry(
                    namespace,
                    redirected.as_str(),
                    follow_final,
                    depth + 1,
                );
            }

            current = next;
            current_path = next_path;
        }

        Ok(NamespaceLookup {
            node: current,
            path: current_path,
        })
    }

    #[allow(dead_code)]
    fn ensure_dir_namespace(&self, fs: &ProcessFsContext, path: &str) -> SysResult<NodeRef> {
        let namespace = fs.namespace();
        let namespace = namespace.lock();
        self.ensure_dir_namespace_locked(&namespace, path)
    }

    fn create_file_namespace(
        &self,
        fs: &ProcessFsContext,
        path: &str,
        mode: u32,
    ) -> SysResult<NodeRef> {
        let namespace = {
            let namespace = fs.namespace();
            namespace.lock().clone()
        };
        self.create_child_namespace_locked(&namespace, path, |parent, name| {
            parent
                .create_file(String::from(name), mode)
                .map_err(SysErr::from)
        })
    }

    fn create_dir_namespace(
        &self,
        fs: &ProcessFsContext,
        path: &str,
        mode: u32,
    ) -> SysResult<NodeRef> {
        let namespace = {
            let namespace = fs.namespace();
            namespace.lock().clone()
        };
        self.create_child_namespace_locked(&namespace, path, |parent, name| {
            parent
                .create_dir(String::from(name), mode)
                .map_err(SysErr::from)
        })
    }

    fn create_symlink_namespace(
        &self,
        fs: &ProcessFsContext,
        path: &str,
        target: &str,
        mode: u32,
    ) -> SysResult<NodeRef> {
        let namespace = {
            let namespace = fs.namespace();
            namespace.lock().clone()
        };
        self.create_child_namespace_locked(&namespace, path, |parent, name| {
            parent
                .create_symlink(String::from(name), String::from(target), mode)
                .map_err(SysErr::from)
        })
    }

    fn create_socket_namespace(
        &self,
        fs: &ProcessFsContext,
        path: &str,
        mode: u32,
    ) -> SysResult<NodeRef> {
        let namespace = {
            let namespace = fs.namespace();
            namespace.lock().clone()
        };
        self.create_child_namespace_locked(&namespace, path, |parent, name| {
            parent
                .create_socket(String::from(name), mode)
                .map_err(SysErr::from)
        })
    }

    fn ensure_dir_namespace_locked(
        &self,
        namespace: &MountNamespace,
        path: &str,
    ) -> SysResult<NodeRef> {
        let normalized = normalize_absolute_path(path);
        if normalized == "/" {
            let root = namespace.root_node();
            if root.kind() != NodeKind::Directory {
                return Err(SysErr::NotDir);
            }
            return Ok(root);
        }

        let mut current = namespace.root_node();
        let mut current_path = String::from("/");
        for component in split_components(normalized.as_str()) {
            let next_path = resolve_namespace_path(current_path.as_str(), component.as_str());
            if let Some(mount) = namespace.lookup_mount(next_path.as_str()) {
                let mount_node = mount.node();
                if mount_node.kind() != NodeKind::Directory {
                    return Err(SysErr::NotDir);
                }
                current = mount_node;
                current_path = next_path;
                continue;
            }

            if let Some(existing) = current.lookup(component.as_str()) {
                let next = if existing.kind() == NodeKind::Symlink {
                    self.lookup_namespace_path(namespace, next_path.as_str(), true, 0)?
                } else {
                    existing
                };
                if next.kind() != NodeKind::Directory {
                    return Err(SysErr::NotDir);
                }
                current = next;
                current_path = next_path;
                continue;
            }

            let next: NodeRef = DirectoryNode::new(component.clone());
            current
                .insert_child(component, next.clone())
                .map_err(SysErr::from)?;
            current = next;
            current_path = next_path;
        }

        Ok(current)
    }

    fn create_child_namespace_locked(
        &self,
        namespace: &MountNamespace,
        path: &str,
        create: impl FnOnce(&NodeRef, &str) -> SysResult<NodeRef>,
    ) -> SysResult<NodeRef> {
        let normalized = normalize_absolute_path(path);
        if normalized == "/" {
            return Err(SysErr::Exists);
        }

        if namespace.lookup_mount(normalized.as_str()).is_some() {
            return Err(SysErr::Exists);
        }

        let parent =
            self.lookup_namespace_path(namespace, parent_path(normalized.as_str()), true, 0)?;
        if parent.kind() != NodeKind::Directory {
            return Err(SysErr::NotDir);
        }

        let name = leaf_name(normalized.as_str());
        if name.is_empty() {
            return Err(SysErr::Inval);
        }
        if parent.lookup(name).is_some() {
            return Err(SysErr::Exists);
        }

        create(&parent, name)
    }

    fn lookup_mount_source(&self, fs: &ProcessFsContext, path: &str) -> SysResult<MountedNode> {
        let namespace_path = resolve_view_path(fs.root_path(), fs.cwd_path(), path);
        let namespace = fs.namespace();
        if let Some(mounted) = namespace.lock().lookup_mount(namespace_path.as_str()) {
            return Ok(mounted);
        }
        let (node, filesystem) = self.lookup_in_with_identity(fs, path, true)?;
        Ok(MountedNode::new(
            node,
            self.bind_fs.clone(),
            self.allocate_mount_device_id(),
            filesystem.statfs,
        ))
    }

    fn allocate_mount_device_id(&self) -> u64 {
        let mut next = self.next_mount_device.lock();
        let device_id = *next;
        *next = next.saturating_add(1);
        device_id
    }

    fn mount_identity(&self, mount: &MountedNode) -> FileSystemIdentity {
        FileSystemIdentity::new(mount.device_id(), mount.statfs())
    }
}

fn parse_shebang(bytes: &[u8]) -> Option<(&str, Option<&str>)> {
    let line = bytes.strip_prefix(b"#!")?;
    let line_end = line
        .iter()
        .position(|byte| *byte == b'\n')
        .unwrap_or(line.len());
    let line = core::str::from_utf8(&line[..line_end]).ok()?.trim();
    if line.is_empty() {
        return None;
    }

    let mut parts = line.split_whitespace();
    let interpreter = parts.next()?;
    let argument = parts.next();
    Some((interpreter, argument))
}
