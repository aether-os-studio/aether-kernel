use core::sync::atomic::{AtomicBool, Ordering};
use x2apic::lapic::{
    IpiDestMode, LocalApic, LocalApicBuildMode, LocalApicBuilder, TimerDivide, TimerMode,
    xapic_base,
};

use crate::boot::MAX_CPUS;
use crate::interrupt::{Trap, TrapFrame};
use crate::io::remap_mmio;
use crate::libs::percpu::PerCpu;
use crate::libs::spin::{LocalIrqDisabled, SpinLock};

pub const PREEMPT_VECTOR: u8 = 0xf1;
pub const TIMER_VECTOR: u8 = 0xf0;
pub const ERROR_VECTOR: u8 = 0xfe;
pub const SPURIOUS_VECTOR: u8 = 0xff;

static LOCAL_APICS: PerCpu<SpinLock<LocalApic, LocalIrqDisabled>, MAX_CPUS> = PerCpu::uninit();
static PREEMPT_IPI_READY: AtomicBool = AtomicBool::new(false);
const APIC_TIMER_CALIBRATION_NS: u64 = 10_000_000;

pub fn init(cpu_index: usize) -> Result<(), &'static str> {
    let apic_base = unsafe { xapic_base() };
    let mapped_apic = remap_mmio(apic_base, 4096).map_err(|_| "failed to remap local APIC")?;
    let mut builder = LocalApicBuilder::new();
    builder
        .mode(LocalApicBuildMode::Auto)
        .timer_vector(TIMER_VECTOR as usize)
        .error_vector(ERROR_VECTOR as usize)
        .spurious_vector(SPURIOUS_VECTOR as usize)
        .timer_mode(TimerMode::Periodic)
        .timer_divide(TimerDivide::Div64)
        .timer_initial(1_000_000)
        .ipi_destination_mode(IpiDestMode::Physical)
        .set_xapic_base(mapped_apic.base() as u64);

    let mut lapic = builder.build()?;
    unsafe {
        lapic.enable();
        lapic.disable_timer();
    }

    LOCAL_APICS
        .init(cpu_index, SpinLock::new(lapic))
        .map_err(|_| "failed to initialize per-cpu local APIC")?;

    Ok(())
}

#[must_use]
pub const fn vector_requires_eoi(vector: u8) -> bool {
    vector >= 32 && vector != SPURIOUS_VECTOR
}

pub fn end_of_interrupt() {
    let _ = with_current_lapic(|lapic| unsafe {
        lapic.end_of_interrupt();
    });
}

pub fn init_preempt_ipi() -> Result<(), &'static str> {
    if PREEMPT_IPI_READY
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Ok(());
    }

    crate::interrupt::register_handler(PREEMPT_VECTOR, preempt_ipi_handler).map_err(|_| {
        PREEMPT_IPI_READY.store(false, Ordering::Release);
        "failed to register preempt IPI handler"
    })
}

#[must_use]
pub fn current_lapic_id() -> Option<u32> {
    with_current_lapic(|lapic| unsafe { lapic.id() }).ok()
}

pub fn program_periodic_timer(initial_count: u32) -> Result<(), &'static str> {
    with_current_lapic(|lapic| unsafe {
        lapic.set_timer_mode(TimerMode::Periodic);
        lapic.set_timer_initial(initial_count);
        lapic.enable_timer();
    })
}

pub fn disable_timer() -> Result<(), &'static str> {
    with_current_lapic(|lapic| unsafe {
        lapic.disable_timer();
    })
}

pub fn kick_cpu(cpu_index: usize) -> Result<(), &'static str> {
    let current_cpu = crate::arch::cpu::current_cpu_index();
    if cpu_index == current_cpu {
        return Ok(());
    }

    let destination = crate::boot::info()
        .cpus
        .as_slice()
        .get(cpu_index)
        .ok_or("invalid target cpu for preempt IPI")?
        .lapic_id;

    with_current_lapic(|lapic| unsafe {
        lapic.send_ipi(PREEMPT_VECTOR, destination);
    })
}

pub fn calibrate_periodic_timer(target_hz: u32) -> Result<u32, &'static str> {
    if target_hz == 0 {
        return Err("target APIC timer frequency must be non-zero");
    }

    let elapsed = with_current_lapic(|lapic| {
        unsafe {
            lapic.disable_timer();
            lapic.set_timer_mode(TimerMode::OneShot);
            lapic.set_timer_initial(u32::MAX);
        }

        crate::arch::timer::hpet::stall_nanos(APIC_TIMER_CALIBRATION_NS)?;

        let remaining = unsafe { lapic.timer_current() };
        unsafe {
            lapic.disable_timer();
        }
        Ok(u32::MAX - remaining)
    })??;

    let periodic_initial =
        (u64::from(elapsed) * 1_000_000_000) / APIC_TIMER_CALIBRATION_NS / u64::from(target_hz);
    if periodic_initial == 0 {
        return Err("calibrated APIC timer initial count is zero");
    }

    Ok(periodic_initial as u32)
}

fn with_current_lapic<R>(f: impl FnOnce(&mut LocalApic) -> R) -> Result<R, &'static str> {
    LOCAL_APICS
        .with(crate::arch::cpu::current_cpu_index(), |lapic| {
            let mut lapic = lapic.lock();
            f(&mut lapic)
        })
        .map_err(|_| "local APIC is not initialized")
}

fn preempt_ipi_handler(_trap: Trap, _frame: &mut TrapFrame) {}
