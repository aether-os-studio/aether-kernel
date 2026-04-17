use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::ptr::{NonNull, addr_of};
use core::sync::atomic::{AtomicBool, Ordering};

use acpi::platform::{AcpiPlatform, PciConfigRegions, ProcessorInfo};
use acpi::sdt::fadt::Fadt;
use acpi::{AcpiError, AcpiTables, Handle, HpetInfo, PciAddress, PhysicalMapping};

pub use acpi::platform::InterruptModel;

#[derive(Debug)]
pub enum AcpiInitError {
    MissingRsdp,
    Acpi(AcpiError),
    Hpet(&'static str),
}

impl From<AcpiError> for AcpiInitError {
    fn from(value: AcpiError) -> Self {
        Self::Acpi(value)
    }
}

#[derive(Clone, Copy)]
struct AetherAcpiHandler;

pub struct AcpiState {
    platform: AcpiPlatform<AetherAcpiHandler>,
    pci_config_regions: Option<PciConfigRegions>,
    hpet_info: Option<HpetInfo>,
    motherboard_implements_8042: bool,
}

impl AcpiState {
    #[must_use]
    pub const fn interrupt_model(&self) -> &InterruptModel {
        &self.platform.interrupt_model
    }

    #[must_use]
    pub const fn processor_info(&self) -> Option<&ProcessorInfo> {
        self.platform.processor_info.as_ref()
    }

    #[must_use]
    pub const fn pci_config_regions(&self) -> Option<&PciConfigRegions> {
        self.pci_config_regions.as_ref()
    }

    #[must_use]
    pub const fn hpet_info(&self) -> Option<&HpetInfo> {
        self.hpet_info.as_ref()
    }

    #[must_use]
    pub const fn motherboard_implements_8042(&self) -> bool {
        self.motherboard_implements_8042
    }

    #[must_use]
    pub fn pcie_config_physical_address(
        &self,
        segment_group: u16,
        bus: u8,
        device: u8,
        function: u8,
    ) -> Option<u64> {
        self.pci_config_regions
            .as_ref()?
            .physical_address(segment_group, bus, device, function)
    }
}

struct AcpiSlot {
    ready: AtomicBool,
    value: UnsafeCell<MaybeUninit<AcpiState>>,
}

unsafe impl Sync for AcpiSlot {}

static ACPI_STATE: AcpiSlot = AcpiSlot {
    ready: AtomicBool::new(false),
    value: UnsafeCell::new(MaybeUninit::uninit()),
};

pub fn init() -> Result<(), AcpiInitError> {
    if ACPI_STATE.ready.load(Ordering::Acquire) {
        return Ok(());
    }

    let rsdp_addr = crate::boot::info()
        .rsdp_addr
        .ok_or(AcpiInitError::MissingRsdp)?;
    let hhdm_offset = crate::boot::hhdm_offset();
    let rsdp_phys = if rsdp_addr >= hhdm_offset {
        rsdp_addr - hhdm_offset
    } else {
        rsdp_addr
    } as usize;
    log::info!("ACPI: parsing tables from RSDP at phys {rsdp_phys:#x}");
    let handler = AetherAcpiHandler;
    let tables = unsafe { AcpiTables::from_rsdp(handler, rsdp_phys)? };
    log::info!("ACPI: basic table walk ready");
    let motherboard_implements_8042 = tables.find_table::<Fadt>().is_some_and(|fadt| unsafe {
        addr_of!(fadt.iapc_boot_arch)
            .read_unaligned()
            .motherboard_implements_8042()
    });
    let pci_config_regions = PciConfigRegions::new(&tables).ok();
    let hpet_info = HpetInfo::new(&tables).ok();
    log::info!(
        "ACPI: mcfg_regions={}, hpet={}, i8042={}",
        pci_config_regions
            .as_ref()
            .map_or(0, |regions| regions.regions.len()),
        hpet_info.is_some(),
        motherboard_implements_8042
    );
    if let Some(hpet_info) = hpet_info.as_ref() {
        crate::arch::timer::hpet::init(hpet_info.base_address as u64)
            .map_err(AcpiInitError::Hpet)?;
        log::info!("ACPI: HPET initialized at {:#x}", hpet_info.base_address);
    }
    let platform = AcpiPlatform::new(tables, handler)?;
    log::info!("ACPI: platform model ready");

    unsafe {
        (*ACPI_STATE.value.get()).write(AcpiState {
            platform,
            pci_config_regions,
            hpet_info,
            motherboard_implements_8042,
        });
    }
    ACPI_STATE.ready.store(true, Ordering::Release);

    Ok(())
}

pub fn info() -> &'static AcpiState {
    while !ACPI_STATE.ready.load(Ordering::Acquire) {
        core::hint::spin_loop();
    }

    unsafe { (*ACPI_STATE.value.get()).assume_init_ref() }
}

#[must_use]
pub fn pcie_config_physical_address(
    segment_group: u16,
    bus: u8,
    device: u8,
    function: u8,
) -> Option<u64> {
    info().pcie_config_physical_address(segment_group, bus, device, function)
}

#[must_use]
pub fn pci_read_u8(address: PciAddress, offset: u16) -> u8 {
    legacy_pci_read_u8(address, offset)
}

#[must_use]
pub fn pci_read_u16(address: PciAddress, offset: u16) -> u16 {
    legacy_pci_read_u16(address, offset)
}

#[must_use]
pub fn pci_read_u32(address: PciAddress, offset: u16) -> u32 {
    legacy_pci_read_u32(address, offset)
}

pub fn pci_write_u8(address: PciAddress, offset: u16, value: u8) {
    legacy_pci_write_u8(address, offset, value);
}

pub fn pci_write_u16(address: PciAddress, offset: u16, value: u16) {
    legacy_pci_write_u16(address, offset, value);
}

pub fn pci_write_u32(address: PciAddress, offset: u16, value: u32) {
    legacy_pci_write_u32(address, offset, value);
}

impl acpi::Handler for AetherAcpiHandler {
    unsafe fn map_physical_region<T>(
        &self,
        physical_address: usize,
        size: usize,
    ) -> PhysicalMapping<Self, T> {
        let virtual_address = crate::boot::phys_to_virt(physical_address as u64) as *mut T;
        PhysicalMapping {
            physical_start: physical_address,
            virtual_start: NonNull::new(virtual_address).expect("ACPI mapping must not be null"),
            region_length: size,
            mapped_length: size,
            handler: *self,
        }
    }

    fn unmap_physical_region<T>(_region: &PhysicalMapping<Self, T>) {}

    fn read_u8(&self, address: usize) -> u8 {
        read_phys(address as u64)
    }

    fn read_u16(&self, address: usize) -> u16 {
        read_phys(address as u64)
    }

    fn read_u32(&self, address: usize) -> u32 {
        read_phys(address as u64)
    }

    fn read_u64(&self, address: usize) -> u64 {
        read_phys(address as u64)
    }

    fn write_u8(&self, address: usize, value: u8) {
        write_phys(address as u64, value);
    }

    fn write_u16(&self, address: usize, value: u16) {
        write_phys(address as u64, value);
    }

    fn write_u32(&self, address: usize, value: u32) {
        write_phys(address as u64, value);
    }

    fn write_u64(&self, address: usize, value: u64) {
        write_phys(address as u64, value);
    }

    fn read_io_u8(&self, port: u16) -> u8 {
        unsafe { crate::arch::io::inb(port) }
    }

    fn read_io_u16(&self, port: u16) -> u16 {
        unsafe { crate::arch::io::inw(port) }
    }

    fn read_io_u32(&self, port: u16) -> u32 {
        unsafe { crate::arch::io::inl(port) }
    }

    fn write_io_u8(&self, port: u16, value: u8) {
        unsafe { crate::arch::io::outb(port, value) }
    }

    fn write_io_u16(&self, port: u16, value: u16) {
        unsafe { crate::arch::io::outw(port, value) }
    }

    fn write_io_u32(&self, port: u16, value: u32) {
        unsafe { crate::arch::io::outl(port, value) }
    }

    fn read_pci_u8(&self, address: PciAddress, offset: u16) -> u8 {
        legacy_pci_read_u8(address, offset)
    }

    fn read_pci_u16(&self, address: PciAddress, offset: u16) -> u16 {
        legacy_pci_read_u16(address, offset)
    }

    fn read_pci_u32(&self, address: PciAddress, offset: u16) -> u32 {
        legacy_pci_read_u32(address, offset)
    }

    fn write_pci_u8(&self, address: PciAddress, offset: u16, value: u8) {
        legacy_pci_write_u8(address, offset, value);
    }

    fn write_pci_u16(&self, address: PciAddress, offset: u16, value: u16) {
        legacy_pci_write_u16(address, offset, value);
    }

    fn write_pci_u32(&self, address: PciAddress, offset: u16, value: u32) {
        legacy_pci_write_u32(address, offset, value);
    }

    fn nanos_since_boot(&self) -> u64 {
        crate::arch::timer::hpet::nanos_since_boot().unwrap_or(0)
    }

    fn stall(&self, microseconds: u64) {
        if crate::arch::timer::hpet::stall_nanos(microseconds.saturating_mul(1_000)).is_ok() {
            return;
        }

        for _ in 0..microseconds.saturating_mul(256) {
            core::hint::spin_loop();
        }
    }

    fn sleep(&self, milliseconds: u64) {
        self.stall(milliseconds.saturating_mul(1_000));
    }

    fn create_mutex(&self) -> Handle {
        Handle(0)
    }

    fn acquire(&self, _mutex: Handle, _timeout: u16) -> Result<(), acpi::aml::AmlError> {
        Ok(())
    }

    fn release(&self, _mutex: Handle) {}
}

fn read_phys<T: Copy>(address: u64) -> T {
    let ptr = crate::boot::phys_to_virt(address) as *const T;
    unsafe { core::ptr::read_volatile(ptr) }
}

fn write_phys<T>(address: u64, value: T) {
    let ptr = crate::boot::phys_to_virt(address) as *mut T;
    unsafe {
        core::ptr::write_volatile(ptr, value);
    }
}

const PCI_CONFIG_ADDRESS_PORT: u16 = 0x0cf8;
const PCI_CONFIG_DATA_PORT: u16 = 0x0cfc;

fn legacy_pci_config_address(address: PciAddress, offset: u16) -> u32 {
    0x8000_0000
        | (u32::from(address.bus()) << 16)
        | (u32::from(address.device()) << 11)
        | (u32::from(address.function()) << 8)
        | u32::from(offset & !0x3)
}

fn legacy_pci_read_u8(address: PciAddress, offset: u16) -> u8 {
    unsafe {
        crate::arch::io::outl(
            PCI_CONFIG_ADDRESS_PORT,
            legacy_pci_config_address(address, offset),
        );
        crate::arch::io::inb(PCI_CONFIG_DATA_PORT + (offset & 0x3))
    }
}

fn legacy_pci_read_u16(address: PciAddress, offset: u16) -> u16 {
    unsafe {
        crate::arch::io::outl(
            PCI_CONFIG_ADDRESS_PORT,
            legacy_pci_config_address(address, offset),
        );
        crate::arch::io::inw(PCI_CONFIG_DATA_PORT + (offset & 0x2))
    }
}

fn legacy_pci_read_u32(address: PciAddress, offset: u16) -> u32 {
    unsafe {
        crate::arch::io::outl(
            PCI_CONFIG_ADDRESS_PORT,
            legacy_pci_config_address(address, offset),
        );
        crate::arch::io::inl(PCI_CONFIG_DATA_PORT)
    }
}

fn legacy_pci_write_u8(address: PciAddress, offset: u16, value: u8) {
    unsafe {
        crate::arch::io::outl(
            PCI_CONFIG_ADDRESS_PORT,
            legacy_pci_config_address(address, offset),
        );
        crate::arch::io::outb(PCI_CONFIG_DATA_PORT + (offset & 0x3), value);
    }
}

fn legacy_pci_write_u16(address: PciAddress, offset: u16, value: u16) {
    unsafe {
        crate::arch::io::outl(
            PCI_CONFIG_ADDRESS_PORT,
            legacy_pci_config_address(address, offset),
        );
        crate::arch::io::outw(PCI_CONFIG_DATA_PORT + (offset & 0x2), value);
    }
}

fn legacy_pci_write_u32(address: PciAddress, offset: u16, value: u32) {
    unsafe {
        crate::arch::io::outl(
            PCI_CONFIG_ADDRESS_PORT,
            legacy_pci_config_address(address, offset),
        );
        crate::arch::io::outl(PCI_CONFIG_DATA_PORT, value);
    }
}
