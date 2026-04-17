use core::arch::asm;

use crate::boot::phys_to_virt;
use crate::mm::{MapFlags, MapSize, PageTableArch, PhysAddr, PhysFrame, VirtAddr};

pub type PageTableEntry = u64;
pub type ArchitecturePageTable = X86_64PageTableArch;

pub struct X86_64PageTableArch;

const ENTRY_COUNT: usize = 512;
const LEVEL_SHIFTS: [usize; 4] = [39, 30, 21, 12];
const PAGE_SIZES: [u64; 4] = [0, 0x4000_0000, 0x20_0000, 0x1000];

const ENTRY_PRESENT: u64 = 1 << 0;
const ENTRY_WRITABLE: u64 = 1 << 1;
const ENTRY_USER: u64 = 1 << 2;
const ENTRY_WRITE_THROUGH: u64 = 1 << 3;
const ENTRY_CACHE_DISABLE: u64 = 1 << 4;
const ENTRY_HUGE: u64 = 1 << 7;
const ENTRY_GLOBAL: u64 = 1 << 8;
const ENTRY_NO_EXECUTE: u64 = 1 << 63;
const ENTRY_ADDR_MASK: u64 = 0x000f_ffff_ffff_f000;

impl PageTableArch for X86_64PageTableArch {
    const LEVELS: usize = 4;
    const ENTRY_COUNT: usize = ENTRY_COUNT;

    fn root_frame() -> PhysFrame {
        let cr3: u64;
        unsafe {
            asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack, preserves_flags));
        }
        PhysFrame::from_start_address(PhysAddr::new(cr3 & ENTRY_ADDR_MASK))
    }

    fn page_size(level: usize) -> Option<u64> {
        PAGE_SIZES.get(level).copied().filter(|size| *size != 0)
    }

    fn leaf_level(size: MapSize) -> Option<usize> {
        match size {
            MapSize::Size1GiB => Some(1),
            MapSize::Size2MiB => Some(2),
            MapSize::Size4KiB => Some(3),
        }
    }

    fn index_of(addr: VirtAddr, level: usize) -> usize {
        ((addr.as_u64() >> LEVEL_SHIFTS[level]) & 0x1ff) as usize
    }

    fn table_entry(frame: PhysFrame, index: usize) -> *mut u64 {
        let table = phys_to_virt(frame.start_address().as_u64()) as *mut u64;
        unsafe { table.add(index) }
    }

    fn is_present(entry: u64) -> bool {
        (entry & ENTRY_PRESENT) != 0
    }

    fn is_leaf(entry: u64, level: usize) -> bool {
        level == 3 || (entry & ENTRY_HUGE) != 0
    }

    fn entry_frame(entry: u64) -> PhysFrame {
        PhysFrame::from_start_address(PhysAddr::new(entry & ENTRY_ADDR_MASK))
    }

    fn make_table(frame: PhysFrame, flags: MapFlags) -> u64 {
        let mut entry = frame.start_address().as_u64() | ENTRY_PRESENT | ENTRY_WRITABLE;
        if flags.contains(MapFlags::USER) {
            entry |= ENTRY_USER;
        }
        entry
    }

    fn make_leaf(frame: PhysFrame, level: usize, flags: MapFlags) -> u64 {
        let mut entry = frame.start_address().as_u64() | ENTRY_PRESENT;

        if flags.contains(MapFlags::WRITE) {
            entry |= ENTRY_WRITABLE;
        }
        if flags.contains(MapFlags::USER) {
            entry |= ENTRY_USER;
        }
        if flags.contains(MapFlags::WRITE_THROUGH) {
            entry |= ENTRY_WRITE_THROUGH;
        }
        if flags.contains(MapFlags::NO_CACHE) {
            entry |= ENTRY_CACHE_DISABLE;
        }
        if flags.contains(MapFlags::GLOBAL) {
            entry |= ENTRY_GLOBAL;
        }
        if !flags.contains(MapFlags::EXECUTE) {
            entry |= ENTRY_NO_EXECUTE;
        }
        if level == 1 || level == 2 {
            entry |= ENTRY_HUGE;
        }

        entry
    }

    fn invalidate(addr: VirtAddr) {
        unsafe {
            asm!("invlpg [{0}]", in(reg) addr.as_u64(), options(nostack, preserves_flags));
        }
    }

    fn invalidate_all() {
        unsafe {
            asm!(
                "mov cr3, rax",
                "mov rax, cr3",
                options(nostack, preserves_flags)
            );
        }
    }
}
