extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use ::acpi::PciAddress;

use crate::acpi;
use crate::io::PciConfigSpace;
use crate::libs::spin::SpinLock;

const PCI_VENDOR_ID: u16 = 0x00;
const PCI_DEVICE_ID: u16 = 0x02;
const PCI_CLASS_REVISION: u16 = 0x08;
const PCI_HEADER_TYPE: u16 = 0x0e;
const PCI_BAR0: u16 = 0x10;
const PCI_SUBSYSTEM_VENDOR_ID: u16 = 0x2c;
const PCI_SUBSYSTEM_ID: u16 = 0x2e;
const PCI_SECONDARY_BUS: u16 = 0x19;
const PCI_SUBORDINATE_BUS: u16 = 0x1a;
const PCI_INTERRUPT_LINE: u16 = 0x3c;
const PCI_INTERRUPT_PIN: u16 = 0x3d;

const PCI_HEADER_TYPE_MASK: u8 = 0x7f;
const PCI_HEADER_TYPE_NORMAL: u8 = 0x00;
const PCI_HEADER_TYPE_BRIDGE: u8 = 0x01;
const PCI_HEADER_TYPE_CARDBUS: u8 = 0x02;

static PCI_DEVICE_CACHE: SpinLock<Option<Vec<PciDeviceInfo>>> = SpinLock::new(None);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PciClassCode {
    pub class: u8,
    pub subclass: u8,
    pub prog_if: u8,
    pub revision: u8,
}

impl PciClassCode {
    #[must_use]
    pub const fn matches(self, class: u8, subclass: u8, prog_if: u8) -> bool {
        self.class == class && self.subclass == subclass && self.prog_if == prog_if
    }

    #[must_use]
    pub const fn is_pci_bridge(self) -> bool {
        self.class == 0x06 && self.subclass == 0x04
    }

    #[must_use]
    pub const fn encoded(self) -> u32 {
        (self.class as u32) << 16 | (self.subclass as u32) << 8 | self.prog_if as u32
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PciDeviceId {
    pub vendor_id: u16,
    pub device_id: u16,
    pub subsystem_vendor_id: Option<u16>,
    pub subsystem_device_id: Option<u16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PciBarKind {
    Memory32,
    Memory64,
    Io,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PciBar {
    pub index: u8,
    pub kind: PciBarKind,
    pub address: u64,
    pub prefetchable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PciDeviceInfo {
    pub address: PciAddress,
    pub path: String,
    pub ids: PciDeviceId,
    pub class: PciClassCode,
    pub header_type: u8,
    pub multifunction: bool,
    pub irq_line: Option<u8>,
    pub irq_pin: Option<u8>,
    pub bars: Vec<PciBar>,
    pub secondary_bus: Option<u8>,
    pub subordinate_bus: Option<u8>,
    pub config_size: usize,
}

impl PciDeviceInfo {
    #[must_use]
    pub const fn slot_name(&self) -> PciAddress {
        self.address
    }

    #[must_use]
    pub const fn is_bridge(&self) -> bool {
        (self.header_type == PCI_HEADER_TYPE_BRIDGE) || self.class.is_pci_bridge()
    }

    #[must_use]
    pub fn name(&self) -> String {
        slot_name(self.address)
    }

    #[must_use]
    pub fn bar(&self, index: usize) -> Option<PciBar> {
        self.bars
            .iter()
            .find(|bar| usize::from(bar.index) == index)
            .copied()
    }

    pub fn config_space(&self) -> Result<PciConfigSpace, &'static str> {
        PciConfigSpace::map(self.address)
    }

    #[must_use]
    pub fn read_config_bytes(&self) -> Vec<u8> {
        let Ok(config) = self.config_space() else {
            return alloc::vec![0xff; self.config_size];
        };

        let mut bytes = Vec::with_capacity(self.config_size);
        for offset in 0..self.config_size {
            bytes.push(config.read_u8(offset as u16).unwrap_or(0xff));
        }
        bytes
    }
}

#[derive(Clone, Copy)]
struct PciBusRange {
    segment: u16,
    start_bus: u8,
    end_bus: u8,
}

#[must_use]
pub fn devices() -> Vec<PciDeviceInfo> {
    let mut cache = PCI_DEVICE_CACHE.lock_irqsave();
    if let Some(devices) = cache.as_ref() {
        return devices.clone();
    }

    let enumerated = enumerate_all();
    *cache = Some(enumerated.clone());
    enumerated
}

pub fn for_each_device(mut callback: impl FnMut(&PciDeviceInfo)) {
    let devices = devices();
    for device in &devices {
        callback(device);
    }
}

fn enumerate_all() -> Vec<PciDeviceInfo> {
    let mut devices = Vec::new();

    for range in bus_ranges() {
        let mut visited = [false; 256];
        for bus in range.start_bus..=range.end_bus {
            if visited[bus as usize] {
                continue;
            }

            let root_path = root_bus_name(range.segment, bus);
            visit_bus(range, bus, root_path.as_str(), &mut visited, &mut devices);
        }
    }

    devices
}

fn bus_ranges() -> Vec<PciBusRange> {
    let Some(regions) = acpi::info().pci_config_regions() else {
        return alloc::vec![PciBusRange {
            segment: 0,
            start_bus: 0,
            end_bus: u8::MAX,
        }];
    };

    regions
        .regions
        .iter()
        .map(|region| PciBusRange {
            segment: region.pci_segment_group,
            start_bus: region.bus_number_start,
            end_bus: region.bus_number_end,
        })
        .collect()
}

fn visit_bus(
    range: PciBusRange,
    bus: u8,
    parent_path: &str,
    visited: &mut [bool; 256],
    devices: &mut Vec<PciDeviceInfo>,
) {
    if bus < range.start_bus || bus > range.end_bus || visited[bus as usize] {
        return;
    }
    visited[bus as usize] = true;

    for device in 0..32u8 {
        let address0 = PciAddress::new(range.segment, bus, device, 0);
        let Ok(config0) = PciConfigSpace::map(address0) else {
            continue;
        };
        let Some(vendor_id) = config0.read_u16(PCI_VENDOR_ID) else {
            continue;
        };
        if vendor_id == 0xffff {
            continue;
        }

        let multifunction = config0
            .read_u8(PCI_HEADER_TYPE)
            .map(|value| (value & 0x80) != 0)
            .unwrap_or(false);
        let functions = if multifunction { 8 } else { 1 };

        for function in 0..functions {
            let address = PciAddress::new(range.segment, bus, device, function);
            let Some(info) = probe_device(address, multifunction, parent_path) else {
                continue;
            };
            let child_path = info.path.clone();
            let secondary_bus = info.secondary_bus;
            let subordinate_bus = info.subordinate_bus;
            let is_bridge = info.is_bridge();

            devices.push(info);

            if is_bridge
                && let (Some(secondary), Some(subordinate)) = (secondary_bus, subordinate_bus)
                && secondary != 0
                && secondary <= subordinate
            {
                visit_bus(range, secondary, child_path.as_str(), visited, devices);
            }
        }
    }
}

fn probe_device(
    address: PciAddress,
    multifunction: bool,
    parent_path: &str,
) -> Option<PciDeviceInfo> {
    let config = PciConfigSpace::map(address).ok()?;
    let vendor_id = config.read_u16(PCI_VENDOR_ID)?;
    if vendor_id == 0xffff {
        return None;
    }

    let class_revision = config.read_u32(PCI_CLASS_REVISION)?;
    let class = PciClassCode {
        class: ((class_revision >> 24) & 0xff) as u8,
        subclass: ((class_revision >> 16) & 0xff) as u8,
        prog_if: ((class_revision >> 8) & 0xff) as u8,
        revision: (class_revision & 0xff) as u8,
    };
    let raw_header_type = config.read_u8(PCI_HEADER_TYPE)?;
    let header_type = raw_header_type & PCI_HEADER_TYPE_MASK;
    let bars = read_bars(&config, header_type);
    let (subsystem_vendor_id, subsystem_device_id) = if header_type == PCI_HEADER_TYPE_NORMAL {
        (
            config.read_u16(PCI_SUBSYSTEM_VENDOR_ID),
            config.read_u16(PCI_SUBSYSTEM_ID),
        )
    } else {
        (None, None)
    };
    let irq_line = config
        .read_u8(PCI_INTERRUPT_LINE)
        .and_then(|value| (value != 0xff).then_some(value));
    let irq_pin = config
        .read_u8(PCI_INTERRUPT_PIN)
        .and_then(|value| (value != 0).then_some(value));
    let secondary_bus = (header_type == PCI_HEADER_TYPE_BRIDGE || class.is_pci_bridge())
        .then(|| config.read_u8(PCI_SECONDARY_BUS))
        .flatten()
        .filter(|value| *value != 0);
    let subordinate_bus = (header_type == PCI_HEADER_TYPE_BRIDGE || class.is_pci_bridge())
        .then(|| config.read_u8(PCI_SUBORDINATE_BUS))
        .flatten()
        .filter(|value| *value != 0);

    Some(PciDeviceInfo {
        address,
        path: alloc::format!("{parent_path}/{}", slot_name(address)),
        ids: PciDeviceId {
            vendor_id,
            device_id: config.read_u16(PCI_DEVICE_ID)?,
            subsystem_vendor_id,
            subsystem_device_id,
        },
        class,
        header_type,
        multifunction,
        irq_line,
        irq_pin,
        bars,
        secondary_bus,
        subordinate_bus,
        config_size: config.config_size(),
    })
}

fn read_bars(config: &PciConfigSpace, header_type: u8) -> Vec<PciBar> {
    let bar_count = match header_type {
        PCI_HEADER_TYPE_NORMAL => 6usize,
        PCI_HEADER_TYPE_BRIDGE => 2usize,
        PCI_HEADER_TYPE_CARDBUS => 1usize,
        _ => 0usize,
    };
    let mut bars = Vec::new();
    let mut index = 0usize;

    while index < bar_count {
        let offset = PCI_BAR0 + (index as u16 * 4);
        let Some(low) = config.read_u32(offset) else {
            break;
        };
        if low == 0 {
            index += 1;
            continue;
        }

        if (low & 0x1) != 0 {
            bars.push(PciBar {
                index: index as u8,
                kind: PciBarKind::Io,
                address: (low & !0x3) as u64,
                prefetchable: false,
            });
            index += 1;
            continue;
        }

        let prefetchable = (low & 0x8) != 0;
        if (low & 0x6) == 0x4 && index + 1 < bar_count {
            let high = config.read_u32(offset + 4).unwrap_or(0) as u64;
            bars.push(PciBar {
                index: index as u8,
                kind: PciBarKind::Memory64,
                address: (high << 32) | u64::from(low & !0xf),
                prefetchable,
            });
            index += 2;
            continue;
        }

        bars.push(PciBar {
            index: index as u8,
            kind: PciBarKind::Memory32,
            address: u64::from(low & !0xf),
            prefetchable,
        });
        index += 1;
    }

    bars
}

fn root_bus_name(segment: u16, bus: u8) -> String {
    alloc::format!("pci{segment:04x}:{bus:02x}")
}

fn slot_name(address: PciAddress) -> String {
    alloc::format!(
        "{:04x}:{:02x}:{:02x}.{}",
        address.segment(),
        address.bus(),
        address.device(),
        address.function()
    )
}
