extern crate alloc;

pub mod nvme;
mod partition;

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use aether_device::{DeviceRegistry, KernelDevice};
use aether_fs::AsyncBlockDevice;

#[derive(Clone)]
pub struct StorageDeviceHandle {
    pub name: String,
    pub device: Arc<dyn AsyncBlockDevice>,
    pub kernel_device: Arc<dyn KernelDevice>,
}

#[derive(Default)]
pub struct DriverInventory {
    storage: Vec<StorageDeviceHandle>,
}

impl DriverInventory {
    pub const fn new() -> Self {
        Self {
            storage: Vec::new(),
        }
    }

    pub fn storage_devices(&self) -> &[StorageDeviceHandle] {
        &self.storage
    }

    fn extend_storage(&mut self, devices: Vec<StorageDeviceHandle>) {
        self.storage.extend(devices);
    }
}

pub fn probe_all(registry: &mut DeviceRegistry) -> DriverInventory {
    let mut inventory = DriverInventory::new();
    let storage = nvme::probe(registry);
    let partitions = partition::probe(registry, &storage);
    inventory.extend_storage(storage);
    inventory.extend_storage(partitions);
    inventory
}
