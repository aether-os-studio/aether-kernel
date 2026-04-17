extern crate alloc;

use alloc::vec::Vec;

use acpi::PciAddress;
use aether_frame::bus::pci::{PciBarKind, devices};
use aether_frame::interrupt::device::{DeviceInterruptMode, configure_pci_message_interrupt};
use aether_frame::io::PciConfigSpace;

#[derive(Debug, Clone, Copy)]
pub struct NvmeControllerInfo {
    pub address: PciAddress,
    pub vendor_id: u16,
    pub device_id: u16,
    pub bar0: u64,
}

pub fn probe_controllers() -> Vec<NvmeControllerInfo> {
    devices()
        .into_iter()
        .filter_map(|device| {
            if !device.class.matches(0x01, 0x08, 0x02) {
                return None;
            }

            let bar0 = device.bar(0)?;
            if !matches!(bar0.kind, PciBarKind::Memory32 | PciBarKind::Memory64) {
                return None;
            }

            Some(NvmeControllerInfo {
                address: device.address,
                vendor_id: device.ids.vendor_id,
                device_id: device.ids.device_id,
                bar0: bar0.address.max(0x1000),
            })
        })
        .collect()
}

pub fn enable_bus_mastering(address: PciAddress) -> Result<(), &'static str> {
    const PCI_COMMAND: usize = 0x04;
    const PCI_COMMAND_IO: u16 = 1 << 0;
    const PCI_COMMAND_MEMORY: u16 = 1 << 1;
    const PCI_COMMAND_BUS_MASTER: u16 = 1 << 2;

    let config = PciConfigSpace::map(address)?;
    let command = read_u16(&config, PCI_COMMAND).ok_or("failed to read PCI command")?;
    write_u16(
        &config,
        PCI_COMMAND,
        command | PCI_COMMAND_IO | PCI_COMMAND_MEMORY | PCI_COMMAND_BUS_MASTER,
    )?;
    Ok(())
}

pub fn enable_message_interrupt(
    address: PciAddress,
    vector: u8,
) -> Result<DeviceInterruptMode, &'static str> {
    let config = PciConfigSpace::map(address)?;
    configure_pci_message_interrupt(&config, vector).map_err(|error| match error {
        aether_frame::interrupt::device::DeviceInterruptError::VectorExhausted => {
            "device interrupt vectors exhausted"
        }
        aether_frame::interrupt::device::DeviceInterruptError::Route(message) => message,
    })
}

fn read_u16(region: &PciConfigSpace, offset: usize) -> Option<u16> {
    region.read_u16(offset as u16)
}

fn write_u16(region: &PciConfigSpace, offset: usize, value: u16) -> Result<(), &'static str> {
    region.write_u16(offset as u16, value)
}
