use core::arch::asm;
use core::mem::size_of;

use crate::boot::MAX_CPUS;
use crate::libs::percpu::PerCpu;

pub(crate) const KERNEL_CODE_SELECTOR: u16 = 0x08;
pub(crate) const KERNEL_DATA_SELECTOR: u16 = 0x10;
pub(crate) const USER_DATA_SELECTOR: u16 = 0x1b;
pub(crate) const USER_CODE_SELECTOR: u16 = 0x23;
const TSS_SELECTOR: u16 = 0x28;

const KERNEL_CODE_DESCRIPTOR: u64 = 0x00af_9a00_0000_ffff;
const KERNEL_DATA_DESCRIPTOR: u64 = 0x00af_9200_0000_ffff;
const USER_DATA_DESCRIPTOR: u64 = 0x00af_f200_0000_ffff;
const USER_CODE_DESCRIPTOR: u64 = 0x00af_fa00_0000_ffff;
const TSS_TYPE_PRESENT: u64 = 0x89;

#[repr(C, packed)]
struct DescriptorTablePointer {
    limit: u16,
    base: u64,
}

#[repr(C, packed)]
struct TaskStateSegment {
    reserved0: u32,
    rsp: [u64; 3],
    reserved1: u64,
    ist: [u64; 7],
    reserved2: u64,
    reserved3: u16,
    io_map_base: u16,
}

impl TaskStateSegment {
    const fn new() -> Self {
        Self {
            reserved0: 0,
            rsp: [0; 3],
            reserved1: 0,
            ist: [0; 7],
            reserved2: 0,
            reserved3: 0,
            io_map_base: size_of::<Self>() as u16,
        }
    }
}

const BASE_GDT: [u64; 7] = [
    0,
    KERNEL_CODE_DESCRIPTOR,
    KERNEL_DATA_DESCRIPTOR,
    USER_DATA_DESCRIPTOR,
    USER_CODE_DESCRIPTOR,
    0,
    0,
];

static TSS: PerCpu<TaskStateSegment, MAX_CPUS> = PerCpu::uninit();
static GDT: PerCpu<[u64; 7], MAX_CPUS> = PerCpu::uninit();

pub fn init(cpu_index: usize) -> Result<(), &'static str> {
    TSS.init(cpu_index, TaskStateSegment::new())
        .map_err(|_| "failed to initialize per-cpu TSS")?;
    GDT.init(cpu_index, BASE_GDT)
        .map_err(|_| "failed to initialize per-cpu GDT")?;

    let tss_base = TSS
        .with(cpu_index, |tss| core::ptr::from_ref(tss) as u64)
        .map_err(|_| "per-cpu TSS is unavailable")?;
    GDT.with_mut(cpu_index, |gdt| install_tss_descriptor(gdt, tss_base))
        .map_err(|_| "per-cpu GDT is unavailable")?;

    let gdtr = GDT
        .with(cpu_index, |gdt| DescriptorTablePointer {
            limit: (size_of::<[u64; 7]>() - 1) as u16,
            base: gdt.as_ptr() as u64,
        })
        .map_err(|_| "per-cpu GDT is unavailable")?;

    unsafe {
        asm!("lgdt [{}]", in(reg) &raw const gdtr, options(readonly, nostack, preserves_flags));
        reload_segments();
        asm!("ltr ax", in("ax") TSS_SELECTOR, options(nostack, preserves_flags));
    }
    Ok(())
}

pub fn set_kernel_stack(stack_top: u64) {
    let _ = TSS.with_mut(crate::arch::cpu::current_cpu_index(), |tss| {
        tss.rsp[0] = stack_top;
    });
}

const fn install_tss_descriptor(gdt: &mut [u64; 7], base: u64) {
    let limit = (size_of::<TaskStateSegment>() - 1) as u64;

    gdt[5] = (limit & 0xffff)
        | ((base & 0x00ff_ffff) << 16)
        | (TSS_TYPE_PRESENT << 40)
        | (((limit >> 16) & 0xf) << 48)
        | (((base >> 24) & 0xff) << 56);
    gdt[6] = base >> 32;
}

unsafe fn reload_segments() {
    asm!(
        "push {kernel_cs}",
        "lea rax, [rip + 2f]",
        "push rax",
        "retfq",
        "2:",
        "mov ax, {kernel_ds}",
        "mov ds, ax",
        "mov es, ax",
        "mov ss, ax",
        "xor eax, eax",
        "mov fs, ax",
        "mov gs, ax",
        kernel_cs = const KERNEL_CODE_SELECTOR as u64,
        kernel_ds = const KERNEL_DATA_SELECTOR,
        lateout("rax") _,
        options(preserves_flags),
    );
}
