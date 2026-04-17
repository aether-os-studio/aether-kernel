use core::sync::atomic::{AtomicU64, Ordering};

use crate::arch::{cpu::current_cpu_index, interrupt::TrapFrame};

static TICKS: AtomicU64 = AtomicU64::new(0);
static mut TICK_HANDLER: Option<fn()> = None;

const DEFAULT_INITIAL_COUNT: u32 = 1_000_000;
const TARGET_TIMER_HZ: u32 = 250;

pub fn init() -> Result<(), &'static str> {
    log::info!("timer: registering interrupt handler");
    crate::interrupt::register_handler(
        crate::arch::interrupt::apic::TIMER_VECTOR,
        timer_interrupt_handler,
    )
    .map_err(|_| "failed to register APIC timer interrupt handler")?;

    log::info!("timer: choosing initial count");
    let initial_count = if crate::arch::timer::hpet::is_initialized() {
        log::info!("timer: calibrating APIC timer via HPET");
        crate::arch::interrupt::apic::calibrate_periodic_timer(TARGET_TIMER_HZ)
            .unwrap_or(DEFAULT_INITIAL_COUNT)
    } else {
        DEFAULT_INITIAL_COUNT
    };

    log::info!("timer: programming periodic timer with initial count {initial_count}");
    crate::arch::interrupt::apic::program_periodic_timer(initial_count)
}

pub fn ticks() -> u64 {
    TICKS.load(Ordering::Acquire)
}

pub fn nanos_since_boot() -> u64 {
    crate::arch::timer::hpet::nanos_since_boot()
        .unwrap_or_else(|| ticks().saturating_mul(1_000_000_000 / TARGET_TIMER_HZ as u64))
}

pub fn unix_time() -> i64 {
    let boot_time = crate::boot::info().boot_time.unwrap_or(0);
    let nanos = nanos_since_boot();
    boot_time.saturating_add((nanos / 1_000_000_000) as i64)
}

pub fn unix_time_nanos() -> (i64, u64) {
    let boot_time = crate::boot::info().boot_time.unwrap_or(0);
    let nanos = nanos_since_boot();
    let secs = (nanos / 1_000_000_000) as i64;
    let subsec_nanos = nanos % 1_000_000_000;
    (boot_time.saturating_add(secs), subsec_nanos)
}

pub fn register_tick_handler(handler: fn()) {
    unsafe {
        TICK_HANDLER = Some(handler);
    }
}

pub fn disable() -> Result<(), &'static str> {
    crate::arch::interrupt::apic::disable_timer()
}

fn timer_interrupt_handler(_trap: crate::interrupt::Trap, _frame: &mut TrapFrame) {
    if current_cpu_index() == 0 {
        TICKS.fetch_add(1, Ordering::AcqRel);
    }
    crate::preempt::request_reschedule();
    if let Some(handler) = unsafe { TICK_HANDLER } {
        handler();
    }
}
