use core::ptr;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::io::MmioRegion;

const HPET_CAPABILITIES: usize = 0x000;
const HPET_CONFIGURATION: usize = 0x010;
const HPET_MAIN_COUNTER: usize = 0x0f0;
const HPET_ENABLE_CNF: u64 = 1 << 0;
const HPET_LEGACY_REPLACEMENT_CNF: u64 = 1 << 1;

static HPET_READY: AtomicBool = AtomicBool::new(false);
static HPET_BASE: AtomicU64 = AtomicU64::new(0);
static HPET_COUNTER_CLOCK_PERIOD_FS: AtomicU64 = AtomicU64::new(0);
static HPET_MAIN_COUNTER_IS_64BIT: AtomicBool = AtomicBool::new(false);

pub fn init(base_address: u64) -> Result<(), &'static str> {
    if HPET_READY.load(Ordering::Acquire) {
        return Ok(());
    }

    let region =
        crate::io::remap_mmio(base_address, 0x400).map_err(|_| "failed to remap HPET registers")?;
    let capabilities = read_u64(&region, HPET_CAPABILITIES);
    let counter_clock_period_fs = (capabilities >> 32) as u32;
    let main_counter_is_64bit = ((capabilities >> 13) & 1) != 0;

    // Align the counter origin with kernel startup so monotonic callers see a
    // true "since boot" timeline rather than firmware uptime.
    disable_hpet(&region);
    disable_legacy_replacement(&region);
    write_u64(&region, HPET_MAIN_COUNTER, 0);
    enable_hpet(&region);

    HPET_BASE.store(region.base() as usize as u64, Ordering::Release);
    HPET_COUNTER_CLOCK_PERIOD_FS.store(counter_clock_period_fs as u64, Ordering::Release);
    HPET_MAIN_COUNTER_IS_64BIT.store(main_counter_is_64bit, Ordering::Release);
    HPET_READY.store(true, Ordering::Release);
    Ok(())
}

pub fn is_initialized() -> bool {
    HPET_READY.load(Ordering::Acquire)
}

#[must_use]
pub fn nanos_since_boot() -> Option<u64> {
    let state = HpetState::load()?;
    Some(counter_to_nanos(
        read_main_counter(&state),
        state.counter_clock_period_fs,
    ))
}

pub fn stall_nanos(duration_ns: u64) -> Result<(), &'static str> {
    let state = HpetState::load().ok_or("HPET is not initialized")?;
    let start = read_main_counter(&state);
    let target = start.saturating_add(nanos_to_counter_ticks(
        duration_ns,
        state.counter_clock_period_fs,
    ));
    while read_main_counter(&state).wrapping_sub(target).cast_signed() < 0 {
        core::hint::spin_loop();
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct HpetState {
    base: u64,
    counter_clock_period_fs: u64,
    main_counter_is_64bit: bool,
}

impl HpetState {
    fn load() -> Option<Self> {
        if !HPET_READY.load(Ordering::Acquire) {
            return None;
        }

        Some(Self {
            base: HPET_BASE.load(Ordering::Acquire),
            counter_clock_period_fs: HPET_COUNTER_CLOCK_PERIOD_FS.load(Ordering::Acquire),
            main_counter_is_64bit: HPET_MAIN_COUNTER_IS_64BIT.load(Ordering::Acquire),
        })
    }
}

fn enable_hpet(region: &MmioRegion) {
    let mut value = read_u64(region, HPET_CONFIGURATION);
    value |= HPET_ENABLE_CNF;
    write_u64(region, HPET_CONFIGURATION, value);
}

fn disable_hpet(region: &MmioRegion) {
    let mut value = read_u64(region, HPET_CONFIGURATION);
    value &= !HPET_ENABLE_CNF;
    write_u64(region, HPET_CONFIGURATION, value);
}

fn disable_legacy_replacement(region: &MmioRegion) {
    let mut value = read_u64(region, HPET_CONFIGURATION);
    value &= !HPET_LEGACY_REPLACEMENT_CNF;
    write_u64(region, HPET_CONFIGURATION, value);
}

fn read_main_counter(state: &HpetState) -> u64 {
    if state.main_counter_is_64bit {
        read_u64_from_base(state.base, HPET_MAIN_COUNTER)
    } else {
        u64::from(read_u32_from_base(state.base, HPET_MAIN_COUNTER))
    }
}

const fn counter_to_nanos(counter: u64, counter_clock_period_fs: u64) -> u64 {
    counter.saturating_mul(counter_clock_period_fs) / 1_000_000
}

const fn nanos_to_counter_ticks(nanos: u64, counter_clock_period_fs: u64) -> u64 {
    nanos
        .saturating_mul(1_000_000)
        .div_ceil(counter_clock_period_fs)
}

fn read_u64(region: &MmioRegion, offset: usize) -> u64 {
    let ptr = region
        .as_ptr::<u64>(offset)
        .expect("HPET MMIO offset must be in range");
    unsafe { ptr::read_volatile(ptr) }
}

fn write_u64(region: &MmioRegion, offset: usize, value: u64) {
    let ptr = region
        .as_ptr::<u64>(offset)
        .expect("HPET MMIO offset must be in range");
    unsafe {
        ptr::write_volatile(ptr, value);
    }
}

fn read_u32_from_base(base: u64, offset: usize) -> u32 {
    let ptr = (base as usize).saturating_add(offset) as *const u32;
    unsafe { ptr::read_volatile(ptr) }
}

fn read_u64_from_base(base: u64, offset: usize) -> u64 {
    let ptr = (base as usize).saturating_add(offset) as *const u64;
    unsafe { ptr::read_volatile(ptr) }
}
