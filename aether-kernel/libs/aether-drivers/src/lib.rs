#![no_std]
#![allow(dead_code)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::extend_with_drain)]
#![allow(clippy::manual_is_multiple_of)]
#![allow(clippy::never_loop)]
#![allow(clippy::result_unit_err)]

extern crate alloc;

pub mod block;
pub mod dma;
pub mod drm;
pub mod i8042;
pub mod input;

use aether_device::DeviceRegistry;

pub use self::block::{DriverInventory, StorageDeviceHandle};
pub use self::dma::DmaRegion;
pub use self::drm::{DrmFile, DrmIoctlError, probe as probe_drm};
pub use self::input::{
    EvdevFile, InputDevice, InputDeviceDescriptor, InputEventSink, LinuxInputEvent,
    register_input_sink,
};

pub fn probe_all(registry: &mut DeviceRegistry) -> DriverInventory {
    let inventory = block::probe_all(registry);
    drm::probe(registry);
    i8042::probe(registry);
    inventory
}
