mod limine;

use core::cell::UnsafeCell;
use core::marker::PhantomData;
use core::mem::MaybeUninit;
use core::slice;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub const MAX_MEMORY_REGIONS: usize = limine::MAX_MEMORY_REGIONS;
pub const MAX_CPUS: usize = limine::MAX_CPUS;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootProtocol {
    Limine,
    Multiboot2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct MemoryRegionKind(pub u64);

impl MemoryRegionKind {
    pub const USABLE: Self = Self(0);
    pub const RESERVED: Self = Self(1);
    pub const ACPI_RECLAIMABLE: Self = Self(3);
    pub const ACPI_NVS: Self = Self(4);
    pub const BAD_MEMORY: Self = Self(5);
    pub const BOOTLOADER_RECLAIMABLE: Self = Self(6);
    pub const EXECUTABLE_AND_MODULES: Self = Self(7);
    pub const FRAMEBUFFER: Self = Self(8);
    pub const ACPI_TABLES: Self = Self(9);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryRegion {
    pub start: u64,
    pub len: u64,
    pub kind: MemoryRegionKind,
}

impl MemoryRegion {
    pub const EMPTY: Self = Self {
        start: 0,
        len: 0,
        kind: MemoryRegionKind::RESERVED,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Initrd {
    pub start: u64,
    pub size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelBitfield {
    pub size: u8,
    pub shift: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelLayout {
    pub red: PixelBitfield,
    pub green: PixelBitfield,
    pub blue: PixelBitfield,
    pub reserved: PixelBitfield,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FramebufferInfo {
    pub base: u64,
    pub size: u64,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub bits_per_pixel: u8,
    pub pixel_layout: PixelLayout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cpu {
    pub processor_id: u32,
    pub lapic_id: u32,
    pub is_bsp: bool,
}

impl Cpu {
    pub const EMPTY: Self = Self {
        processor_id: 0,
        lapic_id: 0,
        is_bsp: false,
    };
}

#[derive(Debug, Clone, Copy)]
pub struct CpuTopology<'a> {
    cpus: &'a [Cpu],
}

impl<'a> CpuTopology<'a> {
    #[must_use]
    pub const fn new(cpus: &'a [Cpu]) -> Self {
        Self { cpus }
    }

    #[must_use]
    pub const fn as_slice(&self) -> &'a [Cpu] {
        self.cpus
    }

    #[must_use]
    pub fn bsp(&self) -> Option<&'a Cpu> {
        self.cpus.iter().find(|cpu| cpu.is_bsp)
    }
}

impl<'a> IntoIterator for &CpuTopology<'a> {
    type Item = &'a Cpu;
    type IntoIter = core::slice::Iter<'a, Cpu>;
    fn into_iter(self) -> Self::IntoIter {
        self.cpus.iter()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct MemoryMap<'a> {
    regions: &'a [MemoryRegion],
}

impl<'a> MemoryMap<'a> {
    #[must_use]
    pub const fn new(regions: &'a [MemoryRegion]) -> Self {
        Self { regions }
    }

    pub fn iter(&self) -> slice::Iter<'a, MemoryRegion> {
        self.regions.iter()
    }
}

impl<'a> IntoIterator for &MemoryMap<'a> {
    type Item = &'a MemoryRegion;
    type IntoIter = core::slice::Iter<'a, MemoryRegion>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct BootInfo<'a> {
    pub protocol: BootProtocol,
    pub command_line: Option<&'a str>,
    pub initrd: Option<Initrd>,
    pub rsdp_addr: Option<u64>,
    pub framebuffer: Option<FramebufferInfo>,
    pub cpus: CpuTopology<'a>,
    pub memory_map: MemoryMap<'a>,
    pub hhdm_offset: u64,
    pub boot_time: Option<i64>,
}

pub trait BootProtocolParser {
    /// Parses raw boot-protocol structures into the framework's stable
    /// boot-time representation.
    ///
    /// # Safety
    /// The caller must provide exclusive access to valid early-boot storage in
    /// `regions` and `cpus`, and the underlying bootloader-owned protocol
    /// structures must remain alive for the duration of parsing.
    unsafe fn parse(
        regions: &'static mut [MemoryRegion; MAX_MEMORY_REGIONS],
        cpus: &'static mut [Cpu; MAX_CPUS],
    ) -> Option<BootInfo<'static>>;
}

struct BootInfoSlot {
    ready: AtomicBool,
    value: UnsafeCell<MaybeUninit<BootInfo<'static>>>,
    _not_sync_by_default: PhantomData<*const ()>,
}

unsafe impl Sync for BootInfoSlot {}

struct MemoryRegionStorage {
    regions: UnsafeCell<[MemoryRegion; MAX_MEMORY_REGIONS]>,
}

unsafe impl Sync for MemoryRegionStorage {}

struct CpuStorage {
    cpus: UnsafeCell<[Cpu; MAX_CPUS]>,
}

unsafe impl Sync for CpuStorage {}

static BOOT_INFO: BootInfoSlot = BootInfoSlot {
    ready: AtomicBool::new(false),
    value: UnsafeCell::new(MaybeUninit::uninit()),
    _not_sync_by_default: PhantomData,
};

static EARLY_MEMORY_REGIONS: MemoryRegionStorage = MemoryRegionStorage {
    regions: UnsafeCell::new([MemoryRegion::EMPTY; MAX_MEMORY_REGIONS]),
};

static EARLY_CPUS: CpuStorage = CpuStorage {
    cpus: UnsafeCell::new([Cpu::EMPTY; MAX_CPUS]),
};

static HHDM_OFFSET: AtomicU64 = AtomicU64::new(0);

pub fn is_ready() -> bool {
    BOOT_INFO.ready.load(Ordering::Acquire)
}

pub fn info() -> &'static BootInfo<'static> {
    while !BOOT_INFO.ready.load(Ordering::Acquire) {
        core::hint::spin_loop();
    }

    unsafe { (*BOOT_INFO.value.get()).assume_init_ref() }
}

pub fn hhdm_offset() -> u64 {
    HHDM_OFFSET.load(Ordering::Acquire)
}

#[must_use]
pub fn phys_to_virt(phys: u64) -> u64 {
    hhdm_offset() + phys
}

#[must_use]
pub fn initrd_bytes() -> Option<&'static [u8]> {
    let initrd = info().initrd?;
    let ptr = initrd.start as *const u8;
    if ptr.is_null() {
        return None;
    }

    Some(unsafe { slice::from_raw_parts(ptr, initrd.size as usize) })
}

pub type SecondaryCpuEntry = fn(cpu_index: usize) -> !;

pub fn start_secondary_cpus(entry: SecondaryCpuEntry) -> Result<usize, &'static str> {
    match info().protocol {
        BootProtocol::Limine => unsafe { limine::start_secondary_cpus(entry) },
        BootProtocol::Multiboot2 => {
            Err("secondary cpu startup is not implemented for this boot protocol")
        }
    }
}

pub(crate) unsafe fn install(info: BootInfo<'static>) {
    HHDM_OFFSET.store(info.hhdm_offset, Ordering::Release);
    unsafe {
        (*BOOT_INFO.value.get()).write(info);
    }
    BOOT_INFO.ready.store(true, Ordering::Release);
}

pub(crate) unsafe fn install_limine() -> bool {
    let Some(info) = (unsafe {
        <limine::LimineProtocol as BootProtocolParser>::parse(
            &mut *EARLY_MEMORY_REGIONS.regions.get(),
            &mut *EARLY_CPUS.cpus.get(),
        )
    }) else {
        return false;
    };

    unsafe {
        install(info);
    }
    true
}

pub(crate) fn limine_base_revision_supported() -> bool {
    limine::base_revision_supported()
}
