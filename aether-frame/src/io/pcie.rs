use acpi::PciAddress;

use super::{MmioRegion, remap_mmio};

const PCI_STATUS: u16 = 0x06;
const PCI_CAPABILITY_LIST: u16 = 0x34;
const PCI_BAR0: u16 = 0x10;
const PCI_HEADER_TYPE: u16 = 0x0e;
const PCI_HEADER_TYPE_MASK: u8 = 0x7f;
const PCI_HEADER_TYPE_NORMAL: u8 = 0x00;
const PCI_HEADER_TYPE_BRIDGE: u8 = 0x01;
const PCI_HEADER_TYPE_CARDBUS: u8 = 0x02;

#[derive(Clone, Copy)]
enum PciConfigBackend {
    Mmio(MmioRegion),
    Legacy,
}

#[derive(Clone, Copy)]
pub struct PciConfigSpace {
    address: PciAddress,
    backend: PciConfigBackend,
}

impl PciConfigSpace {
    pub fn map(address: PciAddress) -> Result<Self, &'static str> {
        let backend = match map_pcie_config(address) {
            Ok(region) => PciConfigBackend::Mmio(region),
            Err(_) if address.segment() == 0 => PciConfigBackend::Legacy,
            Err(error) => return Err(error),
        };
        Ok(Self { address, backend })
    }

    #[must_use]
    pub const fn address(&self) -> PciAddress {
        self.address
    }

    pub fn read_u8(&self, offset: u16) -> Option<u8> {
        match self.backend {
            PciConfigBackend::Mmio(region) => {
                let register = unsafe { region.register::<u8>(offset as usize)? };
                Some(register.read())
            }
            PciConfigBackend::Legacy => {
                if offset >= 256 {
                    return None;
                }
                Some(crate::acpi::pci_read_u8(self.address, offset))
            }
        }
    }

    pub fn read_u16(&self, offset: u16) -> Option<u16> {
        match self.backend {
            PciConfigBackend::Mmio(region) => {
                let register = unsafe { region.register::<u16>(offset as usize)? };
                Some(register.read())
            }
            PciConfigBackend::Legacy => {
                if offset >= 256 {
                    return None;
                }
                Some(crate::acpi::pci_read_u16(self.address, offset))
            }
        }
    }

    pub fn read_u32(&self, offset: u16) -> Option<u32> {
        match self.backend {
            PciConfigBackend::Mmio(region) => {
                let register = unsafe { region.register::<u32>(offset as usize)? };
                Some(register.read())
            }
            PciConfigBackend::Legacy => {
                if offset >= 256 {
                    return None;
                }
                Some(crate::acpi::pci_read_u32(self.address, offset))
            }
        }
    }

    pub fn write_u8(&self, offset: u16, value: u8) -> Result<(), &'static str> {
        match self.backend {
            PciConfigBackend::Mmio(region) => {
                let register = unsafe { region.register::<u8>(offset as usize) }
                    .ok_or("invalid PCI config offset")?;
                register.write(value);
            }
            PciConfigBackend::Legacy => {
                if offset >= 256 {
                    return Err("invalid PCI config offset");
                }
                crate::acpi::pci_write_u8(self.address, offset, value);
            }
        }
        Ok(())
    }

    pub fn write_u16(&self, offset: u16, value: u16) -> Result<(), &'static str> {
        match self.backend {
            PciConfigBackend::Mmio(region) => {
                let register = unsafe { region.register::<u16>(offset as usize) }
                    .ok_or("invalid PCI config offset")?;
                register.write(value);
            }
            PciConfigBackend::Legacy => {
                if offset >= 256 {
                    return Err("invalid PCI config offset");
                }
                crate::acpi::pci_write_u16(self.address, offset, value);
            }
        }
        Ok(())
    }

    pub fn write_u32(&self, offset: u16, value: u32) -> Result<(), &'static str> {
        match self.backend {
            PciConfigBackend::Mmio(region) => {
                let register = unsafe { region.register::<u32>(offset as usize) }
                    .ok_or("invalid PCI config offset")?;
                register.write(value);
            }
            PciConfigBackend::Legacy => {
                if offset >= 256 {
                    return Err("invalid PCI config offset");
                }
                crate::acpi::pci_write_u32(self.address, offset, value);
            }
        }
        Ok(())
    }

    #[must_use]
    pub const fn config_size(&self) -> usize {
        match self.backend {
            PciConfigBackend::Mmio(_) => 4096,
            PciConfigBackend::Legacy => 256,
        }
    }

    #[must_use]
    pub fn find_capability(&self, capability_id: u8) -> Option<u16> {
        let status = self.read_u16(PCI_STATUS)?;
        if (status & (1 << 4)) == 0 {
            return None;
        }

        let mut offset = u16::from(self.read_u8(PCI_CAPABILITY_LIST)?);
        let mut walked = 0usize;
        while offset >= 0x40 && walked < 48 {
            if self.read_u8(offset)? == capability_id {
                return Some(offset);
            }
            offset = u16::from(self.read_u8(offset + 1)? & !0x3);
            if offset == 0 {
                break;
            }
            walked += 1;
        }
        None
    }

    #[must_use]
    pub fn bar_address(&self, index: usize) -> Option<u64> {
        let header_type = self.read_u8(PCI_HEADER_TYPE)? & PCI_HEADER_TYPE_MASK;
        let bar_count = match header_type {
            PCI_HEADER_TYPE_NORMAL => 6usize,
            PCI_HEADER_TYPE_BRIDGE => 2usize,
            PCI_HEADER_TYPE_CARDBUS => 1usize,
            _ => return None,
        };
        if index >= bar_count {
            return None;
        }

        let offset = PCI_BAR0 + (index as u16 * 4);
        let low = self.read_u32(offset)? as u64;
        if (low & 0x1) != 0 {
            return None;
        }
        if (low & 0x6) == 0x4 {
            let high = self.read_u32(offset + 4)? as u64;
            Some((high << 32) | (low & !0xf))
        } else {
            Some(low & !0xf)
        }
    }
}

#[must_use]
pub fn pcie_config_physical_address(address: PciAddress) -> Option<u64> {
    crate::acpi::pcie_config_physical_address(
        address.segment(),
        address.bus(),
        address.device(),
        address.function(),
    )
}

pub fn map_pcie_config(address: PciAddress) -> Result<MmioRegion, &'static str> {
    let phys = pcie_config_physical_address(address).ok_or("PCIe MCFG region not found")?;
    remap_mmio(phys, 4096).map_err(|_| "failed to remap PCIe configuration space")
}
