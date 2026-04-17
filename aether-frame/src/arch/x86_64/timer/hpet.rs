use core::ptr;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::io::MmioRegion;
use crate::libs::spin::SpinLock;

const HPET_CAPABILITIES: usize = 0x000;
const HPET_CONFIGURATION: usize = 0x010;
const HPET_MAIN_COUNTER: usize = 0x0f0;
const HPET_ENABLE_CNF: u64 = 1 << 0;
const HPET_LEGACY_REPLACEMENT_CNF: u64 = 1 << 1;

struct Hpet {
    region: MmioRegion,
    counter_clock_period_fs: u32,
    main_counter_is_64bit: bool,
}

static HPET: SpinLock<Option<Hpet>> = SpinLock::new(None);
static HPET_READY: AtomicBool = AtomicBool::new(false);

pub fn init(base_address: u64) -> Result<(), &'static str> {
    if HPET_READY.load(Ordering::Acquire) {
        return Ok(());
    }

    let region =
        crate::io::remap_mmio(base_address, 0x400).map_err(|_| "failed to remap HPET registers")?;
    let capabilities = read_u64(&region, HPET_CAPABILITIES);
    let counter_clock_period_fs = (capabilities >> 32) as u32;
    let main_counter_is_64bit = ((capabilities >> 13) & 1) != 0;

    let hpet = Hpet {
        region,
        counter_clock_period_fs,
        main_counter_is_64bit,
    };
    hpet.disable_legacy_replacement();
    hpet.enable();

    *HPET.lock_irqsave() = Some(hpet);
    HPET_READY.store(true, Ordering::Release);
    Ok(())
}

pub fn is_initialized() -> bool {
    HPET_READY.load(Ordering::Acquire)
}

#[must_use]
pub fn nanos_since_boot() -> Option<u64> {
    with_hpet(|hpet| hpet.counter_to_nanos(hpet.counter())).ok()
}

pub fn stall_nanos(duration_ns: u64) -> Result<(), &'static str> {
    with_hpet(|hpet| {
        let start = hpet.counter();
        let target = start.saturating_add(hpet.nanos_to_counter_ticks(duration_ns));
        while hpet.counter().wrapping_sub(target).cast_signed() < 0 {
            core::hint::spin_loop();
        }
    })
}

impl Hpet {
    fn enable(&self) {
        let mut value = read_u64(&self.region, HPET_CONFIGURATION);
        value |= HPET_ENABLE_CNF;
        write_u64(&self.region, HPET_CONFIGURATION, value);
    }

    fn disable_legacy_replacement(&self) {
        let mut value = read_u64(&self.region, HPET_CONFIGURATION);
        value &= !HPET_LEGACY_REPLACEMENT_CNF;
        write_u64(&self.region, HPET_CONFIGURATION, value);
    }

    fn counter(&self) -> u64 {
        if self.main_counter_is_64bit {
            read_u64(&self.region, HPET_MAIN_COUNTER)
        } else {
            u64::from(read_u32(&self.region, HPET_MAIN_COUNTER))
        }
    }

    const fn counter_to_nanos(&self, counter: u64) -> u64 {
        counter.saturating_mul(self.counter_clock_period_fs as u64) / 1_000_000
    }

    const fn nanos_to_counter_ticks(&self, nanos: u64) -> u64 {
        nanos
            .saturating_mul(1_000_000)
            .div_ceil(self.counter_clock_period_fs as u64)
    }
}

fn with_hpet<R>(f: impl FnOnce(&Hpet) -> R) -> Result<R, &'static str> {
    let guard = HPET.lock_irqsave();
    let hpet = guard.as_ref().ok_or("HPET is not initialized")?;
    Ok(f(hpet))
}

fn read_u32(region: &MmioRegion, offset: usize) -> u32 {
    let ptr = region
        .as_ptr::<u32>(offset)
        .expect("HPET MMIO offset must be in range");
    unsafe { ptr::read_volatile(ptr) }
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
