#![no_std]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;

use aether_vfs::{FsResult, NodeRef, Vfs};

pub use aether_fs::{
    AsyncBlockDevice, BlockDeviceFile, BlockFuture, BlockGeometry, SyncBlockDevice,
    SyncToAsyncBlockDevice,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceClass {
    Block,
    Display,
    Drm,
    Console,
    MessageBuffer,
    Misc,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceMetadata {
    pub name: String,
    pub class: DeviceClass,
    pub major: u16,
    pub minor: u16,
}

impl DeviceMetadata {
    pub fn new(name: impl Into<String>, class: DeviceClass, major: u16, minor: u16) -> Self {
        Self {
            name: name.into(),
            class,
            major,
            minor,
        }
    }
}

#[derive(Clone)]
pub struct DeviceNode {
    pub path: String,
    pub node: NodeRef,
}

impl DeviceNode {
    pub fn new(path: impl Into<String>, node: NodeRef) -> Self {
        Self {
            path: path.into(),
            node,
        }
    }
}

pub trait KernelDevice: Send + Sync {
    fn metadata(&self) -> DeviceMetadata;
    fn nodes(&self) -> Vec<DeviceNode>;
}

pub trait Driver: Send + Sync {
    fn name(&self) -> &'static str;
    fn probe(&self, registry: &mut DeviceRegistry);
}

#[derive(Default)]
pub struct DeviceRegistry {
    devices: Vec<Arc<dyn KernelDevice>>,
}

impl DeviceRegistry {
    pub const fn new() -> Self {
        Self {
            devices: Vec::new(),
        }
    }

    pub fn register(&mut self, device: Arc<dyn KernelDevice>) {
        let name = device.metadata().name;
        if self
            .devices
            .iter()
            .any(|existing| existing.metadata().name == name)
        {
            return;
        }
        self.devices.push(device);
    }

    pub fn devices(&self) -> &[Arc<dyn KernelDevice>] {
        &self.devices
    }
}

#[derive(Clone)]
pub struct DeviceNamespace {
    root: NodeRef,
}

impl DeviceNamespace {
    pub fn new(root: NodeRef) -> Self {
        Self { root }
    }

    pub fn root(&self) -> NodeRef {
        self.root.clone()
    }

    pub fn lookup(&self, vfs: &Vfs, path: &str) -> Option<NodeRef> {
        vfs.lookup_from(self.root.clone(), path)
    }

    pub fn install(&self, vfs: &Vfs, path: &str, node: NodeRef) -> FsResult<()> {
        vfs.install_at(self.root.clone(), path, node)
    }
}

pub fn default_fbdev_name(index: usize) -> String {
    alloc::format!("fb{index}")
}

pub fn default_console_name(index: usize) -> String {
    alloc::format!("tty{index}")
}

pub fn default_console_alias() -> String {
    "console".to_string()
}

pub fn default_kmsg_name() -> String {
    "kmsg".to_string()
}
