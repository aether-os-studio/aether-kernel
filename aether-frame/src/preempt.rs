use core::sync::atomic::{AtomicU32, Ordering};

use crate::boot::MAX_CPUS;
use crate::libs::percpu::PerCpu;

const NEED_RESCHED_MASK: u32 = 1 << 31;
const DISABLE_COUNT_MASK: u32 = NEED_RESCHED_MASK - 1;

struct PreemptState {
    info: AtomicU32,
}

impl PreemptState {
    const fn new() -> Self {
        Self {
            info: AtomicU32::new(NEED_RESCHED_MASK),
        }
    }
}

static PREEMPT_STATE: PerCpu<PreemptState, MAX_CPUS> = PerCpu::uninit();

#[must_use]
#[derive(Debug)]
pub struct DisabledPreemptGuard {
    active: bool,
}

impl DisabledPreemptGuard {
    fn new() -> Self {
        if let Some(state) = current_state() {
            let previous = state.info.fetch_add(1, Ordering::AcqRel);
            assert!(
                (previous & DISABLE_COUNT_MASK) != DISABLE_COUNT_MASK,
                "preempt disable count overflow"
            );
            Self { active: true }
        } else {
            Self { active: false }
        }
    }
}

impl Drop for DisabledPreemptGuard {
    fn drop(&mut self) {
        if !self.active {
            return;
        }

        let state = current_state().expect("preempt guard dropped without cpu-local state");
        let previous = state.info.fetch_sub(1, Ordering::AcqRel);
        assert!(
            (previous & DISABLE_COUNT_MASK) != 0,
            "preempt disable count underflow"
        );
    }
}

pub fn init_for_cpu(cpu_index: usize) -> Result<(), &'static str> {
    PREEMPT_STATE
        .init(cpu_index, PreemptState::new())
        .map_err(|_| "failed to initialize per-cpu preempt state")
}

pub fn disable_preempt() -> DisabledPreemptGuard {
    DisabledPreemptGuard::new()
}

pub fn disable() {
    let guard = disable_preempt();
    core::mem::forget(guard);
}

pub fn enable() {
    let Some(state) = current_state() else {
        return;
    };
    let previous = state.info.fetch_sub(1, Ordering::AcqRel);
    assert!(
        (previous & DISABLE_COUNT_MASK) != 0,
        "preempt disable count underflow"
    );
}

pub fn count() -> usize {
    current_state()
        .map(|state| (state.info.load(Ordering::Acquire) & DISABLE_COUNT_MASK) as usize)
        .unwrap_or(0)
}

pub fn is_disabled() -> bool {
    count() != 0
}

pub fn request_reschedule() {
    if let Some(state) = current_state() {
        state.info.fetch_and(!NEED_RESCHED_MASK, Ordering::AcqRel);
    }
}

pub fn request_reschedule_cpu(cpu_index: usize) {
    if let Ok(state) = PREEMPT_STATE.get(cpu_index) {
        state.info.fetch_and(!NEED_RESCHED_MASK, Ordering::AcqRel);
    }
}

pub fn need_resched() -> bool {
    current_state()
        .map(|state| (state.info.load(Ordering::Acquire) & NEED_RESCHED_MASK) == 0)
        .unwrap_or(false)
}

pub fn should_preempt() -> bool {
    current_state()
        .map(|state| state.info.load(Ordering::Acquire) == 0)
        .unwrap_or(false)
}

pub fn take_need_resched() -> bool {
    current_state()
        .map(|state| {
            (state.info.fetch_or(NEED_RESCHED_MASK, Ordering::AcqRel) & NEED_RESCHED_MASK) == 0
        })
        .unwrap_or(false)
}

pub fn clear_need_resched() {
    if let Some(state) = current_state() {
        state.info.fetch_or(NEED_RESCHED_MASK, Ordering::AcqRel);
    }
}

pub fn preemptible() -> bool {
    !is_disabled() && crate::interrupt::are_enabled()
}

fn current_state() -> Option<&'static PreemptState> {
    if !crate::boot::is_ready() {
        return None;
    }

    PREEMPT_STATE
        .get(crate::arch::cpu::current_cpu_index())
        .ok()
}
