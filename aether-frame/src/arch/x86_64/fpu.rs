use core::arch::asm;
use core::arch::x86_64::__cpuid_count;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use crate::boot::MAX_CPUS;
use crate::libs::percpu::PerCpu;

const CR0_MONITOR_COPROCESSOR: u64 = 1 << 1;
const CR0_EMULATION: u64 = 1 << 2;
const CR0_TASK_SWITCHED: u64 = 1 << 3;

const CR4_OSFXSR: u64 = 1 << 9;
const CR4_OSXMMEXCPT: u64 = 1 << 10;
const CR4_OSXSAVE: u64 = 1 << 18;

const CPUID_FEATURES_LEAF: u32 = 1;
const CPUID_XSAVE_LEAF: u32 = 0x0d;
const CPUID_FEATURES_ECX_XSAVE: u32 = 1 << 26;
const CPUID_FEATURES_ECX_AVX: u32 = 1 << 28;
const CPUID_FEATURES_EDX_FXSR: u32 = 1 << 24;

const XCR0_X87: u64 = 1 << 0;
const XCR0_SSE: u64 = 1 << 1;
const XCR0_AVX: u64 = 1 << 2;

const DEFAULT_X87_CONTROL_WORD: u16 = 0x037f;
const DEFAULT_MXCSR: u32 = 0x1f80;

const FXSAVE_AREA_SIZE: usize = 512;
const XSAVE_HEADER_OFFSET: usize = 512;
const MAX_FPU_STATE_SIZE: usize = 4096;

const FCW_OFFSET: usize = 0;
const MXCSR_OFFSET: usize = 24;
const XSTATE_BV_OFFSET: usize = XSAVE_HEADER_OFFSET;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FpuConfig {
    xsave_enabled: bool,
    xsave_mask: u64,
    state_size: usize,
}

impl FpuConfig {
    const fn fxsave() -> Self {
        Self {
            xsave_enabled: false,
            xsave_mask: 0,
            state_size: FXSAVE_AREA_SIZE,
        }
    }
}

static CONFIG_READY: AtomicBool = AtomicBool::new(false);
static XSAVE_ENABLED: AtomicBool = AtomicBool::new(false);
static XSAVE_MASK: AtomicU64 = AtomicU64::new(0);
static STATE_SIZE: AtomicUsize = AtomicUsize::new(FXSAVE_AREA_SIZE);
static KERNEL_INTERRUPT_STATES: PerCpu<FpuState, MAX_CPUS> = PerCpu::uninit();

#[repr(C, align(64))]
pub struct FpuState {
    bytes: [u8; MAX_FPU_STATE_SIZE],
}

impl FpuState {
    pub(crate) const fn new() -> Self {
        Self {
            bytes: [0; MAX_FPU_STATE_SIZE],
        }
    }

    pub(crate) fn initialize(&mut self) {
        self.bytes.fill(0);
        self.write_u16(FCW_OFFSET, DEFAULT_X87_CONTROL_WORD);
        self.write_u32(MXCSR_OFFSET, DEFAULT_MXCSR);

        let config = config();
        if config.xsave_enabled && config.state_size > XSTATE_BV_OFFSET {
            self.write_u64(XSTATE_BV_OFFSET, config.xsave_mask);
        }
    }

    const fn as_ptr(&self) -> *const u8 {
        self.bytes.as_ptr()
    }

    const fn as_mut_ptr(&mut self) -> *mut u8 {
        self.bytes.as_mut_ptr()
    }

    fn write_u16(&mut self, offset: usize, value: u16) {
        self.bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn write_u32(&mut self, offset: usize, value: u32) {
        self.bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn write_u64(&mut self, offset: usize, value: u64) {
        self.bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }

    pub(crate) fn copy_from(&mut self, other: &Self) {
        self.bytes.copy_from_slice(&other.bytes);
    }

    pub fn copy_prefix_to(&self, dest: &mut [u8]) -> usize {
        let len = core::cmp::min(dest.len(), self.active_len());
        dest[..len].copy_from_slice(&self.bytes[..len]);
        len
    }

    pub fn restore_prefix_from(&mut self, src: &[u8]) -> usize {
        let len = core::cmp::min(src.len(), self.active_len());
        self.bytes[..len].copy_from_slice(&src[..len]);
        len
    }

    pub fn active_len(&self) -> usize {
        config().state_size
    }
}

impl Default for FpuState {
    fn default() -> Self {
        let mut state = Self::new();
        state.initialize();
        state
    }
}

pub fn init_for_cpu(cpu_index: usize) -> Result<(), &'static str> {
    let features = __cpuid_count(CPUID_FEATURES_LEAF, 0);
    if (features.edx & CPUID_FEATURES_EDX_FXSR) == 0 {
        return Err("CPU does not support fxsave/fxrstor");
    }

    let mut cr0 = read_cr0();
    let mut cr4 = read_cr4();

    cr0 &= !(CR0_EMULATION | CR0_TASK_SWITCHED);
    cr0 |= CR0_MONITOR_COPROCESSOR;
    write_cr0(cr0);

    cr4 |= CR4_OSFXSR | CR4_OSXMMEXCPT;

    let mut selected = FpuConfig::fxsave();
    if (features.ecx & CPUID_FEATURES_ECX_XSAVE) != 0 {
        cr4 |= CR4_OSXSAVE;
        write_cr4(cr4);

        let xsave_leaf = __cpuid_count(CPUID_XSAVE_LEAF, 0);
        let supported_mask = (u64::from(xsave_leaf.edx) << 32) | u64::from(xsave_leaf.eax);
        let mut xcr0_mask = (XCR0_X87 | XCR0_SSE) & supported_mask;

        if xcr0_mask == (XCR0_X87 | XCR0_SSE) {
            if (features.ecx & CPUID_FEATURES_ECX_AVX) != 0 && (supported_mask & XCR0_AVX) != 0 {
                xcr0_mask |= XCR0_AVX;
            }

            write_xcr0(xcr0_mask);
            let enabled_leaf = __cpuid_count(CPUID_XSAVE_LEAF, 0);
            let xsave_area_size = usize::max(enabled_leaf.ebx as usize, FXSAVE_AREA_SIZE);
            if xsave_area_size <= MAX_FPU_STATE_SIZE {
                selected = FpuConfig {
                    xsave_enabled: true,
                    xsave_mask: xcr0_mask,
                    state_size: xsave_area_size,
                };
            } else {
                log::warn!(
                    "cpu {cpu_index} xsave area {xsave_area_size} exceeds {MAX_FPU_STATE_SIZE} bytes, falling back to fxsave"
                );
                cr4 &= !CR4_OSXSAVE;
            }
        } else {
            log::warn!(
                "cpu {cpu_index} xsave leaf misses required x87/sse state, falling back to fxsave"
            );
            cr4 &= !CR4_OSXSAVE;
        }
    }

    write_cr4(cr4);
    publish_config(cpu_index, selected);
    KERNEL_INTERRUPT_STATES
        .init(cpu_index, FpuState::default())
        .map_err(|_| "failed to initialize per-cpu kernel interrupt fpu state")?;
    Ok(())
}

pub fn save(state: &mut FpuState) {
    let config = config();
    if config.xsave_enabled {
        let eax = config.xsave_mask as u32;
        let edx = (config.xsave_mask >> 32) as u32;
        unsafe {
            asm!(
                "xsave [{}]",
                in(reg) state.as_mut_ptr(),
                in("eax") eax,
                in("edx") edx,
                options(nostack, preserves_flags),
            );
        }
        return;
    }

    unsafe {
        asm!(
            "fxsave [{}]",
            in(reg) state.as_mut_ptr(),
            options(nostack, preserves_flags),
        );
    }
}

pub fn restore(state: &FpuState) {
    let config = config();
    if config.xsave_enabled {
        let eax = config.xsave_mask as u32;
        let edx = (config.xsave_mask >> 32) as u32;
        unsafe {
            asm!(
                "xrstor [{}]",
                in(reg) state.as_ptr(),
                in("eax") eax,
                in("edx") edx,
                options(nostack, preserves_flags),
            );
        }
        return;
    }

    unsafe {
        asm!(
            "fxrstor [{}]",
            in(reg) state.as_ptr(),
            options(nostack, preserves_flags),
        );
    }
}

pub fn save_kernel_interrupt_state() {
    let _ = KERNEL_INTERRUPT_STATES.with_mut(crate::arch::cpu::current_cpu_index(), save);
}

pub fn restore_kernel_interrupt_state() {
    let _ = KERNEL_INTERRUPT_STATES.with(crate::arch::cpu::current_cpu_index(), restore);
}

fn publish_config(cpu_index: usize, candidate: FpuConfig) {
    let previous = config();
    if !CONFIG_READY.swap(true, Ordering::AcqRel) {
        XSAVE_ENABLED.store(candidate.xsave_enabled, Ordering::Release);
        XSAVE_MASK.store(candidate.xsave_mask, Ordering::Release);
        STATE_SIZE.store(candidate.state_size, Ordering::Release);
        log::info!(
            "cpu {} fpu initialized: xsave={}, mask={:#x}, state_size={}",
            cpu_index,
            candidate.xsave_enabled,
            candidate.xsave_mask,
            candidate.state_size
        );
        return;
    }

    if previous != candidate {
        log::warn!("cpu {cpu_index} fpu config mismatch: global={previous:?}, local={candidate:?}");
    }
}

fn config() -> FpuConfig {
    if !CONFIG_READY.load(Ordering::Acquire) {
        return FpuConfig::fxsave();
    }

    FpuConfig {
        xsave_enabled: XSAVE_ENABLED.load(Ordering::Acquire),
        xsave_mask: XSAVE_MASK.load(Ordering::Acquire),
        state_size: STATE_SIZE.load(Ordering::Acquire),
    }
}

fn read_cr0() -> u64 {
    let value: u64;
    unsafe {
        asm!("mov {}, cr0", out(reg) value, options(nomem, nostack, preserves_flags));
    }
    value
}

fn write_cr0(value: u64) {
    unsafe {
        asm!("mov cr0, {}", in(reg) value, options(nomem, nostack, preserves_flags));
    }
}

fn read_cr4() -> u64 {
    let value: u64;
    unsafe {
        asm!("mov {}, cr4", out(reg) value, options(nomem, nostack, preserves_flags));
    }
    value
}

fn write_cr4(value: u64) {
    unsafe {
        asm!("mov cr4, {}", in(reg) value, options(nomem, nostack, preserves_flags));
    }
}

fn write_xcr0(value: u64) {
    let eax = value as u32;
    let edx = (value >> 32) as u32;
    unsafe {
        asm!(
            "xsetbv",
            in("ecx") 0u32,
            in("eax") eax,
            in("edx") edx,
            options(nostack, preserves_flags),
        );
    }
}
