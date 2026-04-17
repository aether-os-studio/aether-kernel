use crate::io::{PciConfigSpace, remap_mmio};

const PCI_COMMAND: u16 = 0x04;
const PCI_COMMAND_INTERRUPT_DISABLE: u16 = 1 << 10;

const PCI_CAP_ID_MSI: u8 = 0x05;
const PCI_CAP_ID_MSIX: u8 = 0x11;

const MSI_FLAGS_ENABLE: u16 = 1 << 0;
const MSI_FLAGS_64BIT: u16 = 1 << 7;
const MSI_FLAGS_MULTI_MESSAGE_ENABLE_MASK: u16 = 0x7 << 4;

const MSIX_FLAGS_ENABLE: u16 = 1 << 15;
const MSIX_FLAGS_FUNCTION_MASK: u16 = 1 << 14;
const MSIX_TABLE_BIR_MASK: u32 = 0x7;
const MSIX_TABLE_OFFSET_MASK: u32 = !MSIX_TABLE_BIR_MASK;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PciInterruptMode {
    Msi,
    MsiX,
}

pub fn enable_best_available(
    config: &PciConfigSpace,
    vector: u8,
) -> Result<PciInterruptMode, &'static str> {
    // Prefer MSI first.
    //
    // Our current MSI path is simpler and avoids the extra MSI-X table BAR
    // programming step. That makes it a safer default while the MSI-X path is
    // still being validated on real workloads.
    if let Ok(()) = enable_msi(config, vector) {
        return Ok(PciInterruptMode::Msi);
    }
    enable_msix(config, vector)?;
    Ok(PciInterruptMode::MsiX)
}

pub fn enable_msi(config: &PciConfigSpace, vector: u8) -> Result<(), &'static str> {
    let capability = config
        .find_capability(PCI_CAP_ID_MSI)
        .ok_or("PCI MSI capability not found")?;
    let destination = super::apic::current_lapic_id().ok_or("local APIC id unavailable")?;
    let control = config
        .read_u16(capability + 2)
        .ok_or("failed to read MSI control")?;
    let is_64bit = (control & MSI_FLAGS_64BIT) != 0;

    config.write_u32(capability + 4, message_address(destination))?;
    let data_offset = if is_64bit {
        config.write_u32(capability + 8, 0)?;
        capability + 12
    } else {
        capability + 8
    };
    config.write_u16(data_offset, message_data(vector))?;
    config.write_u16(
        capability + 2,
        (control & !MSI_FLAGS_MULTI_MESSAGE_ENABLE_MASK) | MSI_FLAGS_ENABLE,
    )?;
    disable_legacy_interrupt(config)?;
    Ok(())
}

pub fn enable_msix(config: &PciConfigSpace, vector: u8) -> Result<(), &'static str> {
    let capability = config
        .find_capability(PCI_CAP_ID_MSIX)
        .ok_or("PCI MSI-X capability not found")?;
    let destination = super::apic::current_lapic_id().ok_or("local APIC id unavailable")?;
    let control = config
        .read_u16(capability + 2)
        .ok_or("failed to read MSI-X control")?;
    let table_info = config
        .read_u32(capability + 4)
        .ok_or("failed to read MSI-X table info")?;
    let bir = (table_info & MSIX_TABLE_BIR_MASK) as usize;
    let table_offset = u64::from(table_info & MSIX_TABLE_OFFSET_MASK);
    let bar = config
        .bar_address(bir)
        .ok_or("failed to resolve MSI-X table BAR")?;

    config.write_u16(
        capability + 2,
        control | MSIX_FLAGS_ENABLE | MSIX_FLAGS_FUNCTION_MASK,
    )?;

    let table = remap_mmio(bar + table_offset, 16).map_err(|_| "failed to map MSI-X table")?;
    let address_low = unsafe { table.register::<u32>(0) }.ok_or("invalid MSI-X table layout")?;
    let address_high = unsafe { table.register::<u32>(4) }.ok_or("invalid MSI-X table layout")?;
    let data = unsafe { table.register::<u32>(8) }.ok_or("invalid MSI-X table layout")?;
    let vector_control =
        unsafe { table.register::<u32>(12) }.ok_or("invalid MSI-X table layout")?;

    vector_control.write(1);
    address_low.write(message_address(destination));
    address_high.write(0);
    data.write(u32::from(message_data(vector)));
    vector_control.write(0);

    config.write_u16(
        capability + 2,
        (control | MSIX_FLAGS_ENABLE) & !MSIX_FLAGS_FUNCTION_MASK,
    )?;
    disable_legacy_interrupt(config)?;
    Ok(())
}

fn disable_legacy_interrupt(config: &PciConfigSpace) -> Result<(), &'static str> {
    let command = config
        .read_u16(PCI_COMMAND)
        .ok_or("failed to read PCI command register")?;
    config.write_u16(PCI_COMMAND, command | PCI_COMMAND_INTERRUPT_DISABLE)
}

const fn message_address(destination_lapic_id: u32) -> u32 {
    0xfee0_0000 | ((destination_lapic_id & 0xff) << 12)
}

const fn message_data(vector: u8) -> u16 {
    vector as u16
}
