use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::boot::MAX_CPUS;
use crate::libs::percpu::PerCpu;

struct PreemptState {
    count: AtomicUsize,
    need_resched: AtomicBool,
}

impl PreemptState {
    const fn new() -> Self {
        Self {
            count: AtomicUsize::new(0),
            need_resched: AtomicBool::new(false),
        }
    }
}

static PREEMPT_STATE: PerCpu<PreemptState, MAX_CPUS> = PerCpu::uninit();

pub fn init_for_cpu(cpu_index: usize) -> Result<(), &'static str> {
    PREEMPT_STATE
        .init(cpu_index, PreemptState::new())
        .map_err(|_| "failed to initialize per-cpu preempt state")
}

pub fn disable() {
    if let Some(state) = current_state() {
        state.count.fetch_add(1, Ordering::AcqRel);
    }
}

pub fn enable() {
    let Some(state) = current_state() else {
        return;
    };
    let previous = state.count.fetch_sub(1, Ordering::AcqRel);
    assert!(previous != 0, "preempt_count underflow");
}

pub fn count() -> usize {
    current_state()
        .map(|state| state.count.load(Ordering::Acquire))
        .unwrap_or(0)
}

pub fn is_disabled() -> bool {
    count() != 0
}

pub fn request_reschedule() {
    if let Some(state) = current_state() {
        state.need_resched.store(true, Ordering::Release);
    }
}

pub fn request_reschedule_cpu(cpu_index: usize) {
    if let Ok(state) = PREEMPT_STATE.get(cpu_index) {
        state.need_resched.store(true, Ordering::Release);
    }
}

pub fn need_resched() -> bool {
    current_state()
        .map(|state| state.need_resched.load(Ordering::Acquire))
        .unwrap_or(false)
}

pub fn take_need_resched() -> bool {
    current_state()
        .map(|state| state.need_resched.swap(false, Ordering::AcqRel))
        .unwrap_or(false)
}

pub fn clear_need_resched() {
    if let Some(state) = current_state() {
        state.need_resched.store(false, Ordering::Release);
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
