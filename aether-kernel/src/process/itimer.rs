extern crate alloc;

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use aether_frame::libs::spin::SpinLock;

use super::Pid;
use crate::signal::{SIGALRM, SignalInfo};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RealTimerState {
    deadline_nanos: Option<u64>,
    interval_nanos: u64,
}

#[derive(Default)]
struct RealTimerRegistry {
    timers: BTreeMap<Pid, RealTimerState>,
    deadlines: BTreeMap<u64, BTreeSet<Pid>>,
}

static REAL_TIMER_REGISTRY: SpinLock<RealTimerRegistry> = SpinLock::new(RealTimerRegistry {
    timers: BTreeMap::new(),
    deadlines: BTreeMap::new(),
});
static NEXT_REAL_TIMER_DEADLINE: AtomicU64 = AtomicU64::new(u64::MAX);

fn publish_next_deadline(registry: &RealTimerRegistry) {
    NEXT_REAL_TIMER_DEADLINE.store(
        registry
            .deadlines
            .first_key_value()
            .map(|(&deadline, _)| deadline)
            .unwrap_or(u64::MAX),
        Ordering::Release,
    );
}

fn remove_deadline(registry: &mut RealTimerRegistry, tgid: Pid, deadline_nanos: u64) {
    if let Some(waiters) = registry.deadlines.get_mut(&deadline_nanos) {
        waiters.remove(&tgid);
        if waiters.is_empty() {
            registry.deadlines.remove(&deadline_nanos);
        }
    }
}

fn add_deadline(registry: &mut RealTimerRegistry, tgid: Pid, deadline_nanos: u64) {
    registry
        .deadlines
        .entry(deadline_nanos)
        .or_default()
        .insert(tgid);
}

pub(crate) fn next_deadline() -> Option<u64> {
    match NEXT_REAL_TIMER_DEADLINE.load(Ordering::Acquire) {
        u64::MAX => None,
        deadline => Some(deadline),
    }
}

pub(crate) fn deadline_due(current_nanos: u64) -> bool {
    next_deadline().is_some_and(|deadline| deadline <= current_nanos)
}

pub(crate) fn read_real_timer(tgid: Pid, now_nanos: u64) -> (u64, u64) {
    let registry = REAL_TIMER_REGISTRY.lock();
    let Some(timer) = registry.timers.get(&tgid) else {
        return (0, 0);
    };
    (
        timer
            .deadline_nanos
            .map(|deadline| deadline.saturating_sub(now_nanos))
            .unwrap_or(0),
        timer.interval_nanos,
    )
}

pub(crate) fn set_real_timer(
    tgid: Pid,
    now_nanos: u64,
    value_nanos: u64,
    interval_nanos: u64,
) -> (u64, u64) {
    let mut registry = REAL_TIMER_REGISTRY.lock();
    let old = registry.timers.get(&tgid).copied();
    if let Some(old_timer) = old
        && let Some(deadline_nanos) = old_timer.deadline_nanos
    {
        remove_deadline(&mut registry, tgid, deadline_nanos);
    }

    let new_timer = RealTimerState {
        deadline_nanos: (value_nanos != 0).then(|| now_nanos.saturating_add(value_nanos)),
        interval_nanos,
    };

    if new_timer.deadline_nanos.is_none() && new_timer.interval_nanos == 0 {
        registry.timers.remove(&tgid);
    } else {
        if let Some(deadline_nanos) = new_timer.deadline_nanos {
            add_deadline(&mut registry, tgid, deadline_nanos);
        }
        registry.timers.insert(tgid, new_timer);
    }

    publish_next_deadline(&registry);
    (
        old.and_then(|timer| timer.deadline_nanos)
            .map(|deadline| deadline.saturating_sub(now_nanos))
            .unwrap_or(0),
        old.map(|timer| timer.interval_nanos).unwrap_or(0),
    )
}

pub(crate) fn thread_group_reaped(tgid: Pid) {
    let mut registry = REAL_TIMER_REGISTRY.lock();
    if let Some(timer) = registry.timers.remove(&tgid)
        && let Some(deadline_nanos) = timer.deadline_nanos
    {
        remove_deadline(&mut registry, tgid, deadline_nanos);
    }
    publish_next_deadline(&registry);
}

pub(crate) fn take_expired(current_nanos: u64) -> Vec<(Pid, SignalInfo)> {
    let mut registry = REAL_TIMER_REGISTRY.lock();
    let mut expired = Vec::new();

    loop {
        let Some((&deadline_nanos, _)) = registry.deadlines.first_key_value() else {
            break;
        };
        if deadline_nanos > current_nanos {
            break;
        }

        let Some(tgids) = registry.deadlines.remove(&deadline_nanos) else {
            continue;
        };

        for tgid in tgids {
            let Some(timer) = registry.timers.get(&tgid).copied() else {
                continue;
            };
            if timer.deadline_nanos != Some(deadline_nanos) {
                continue;
            }

            let next_deadline = if timer.interval_nanos == 0 {
                registry.timers.remove(&tgid);
                None
            } else {
                let interval = timer.interval_nanos as u128;
                let next = deadline_nanos as u128
                    + (((current_nanos.saturating_sub(deadline_nanos)) as u128 / interval) + 1)
                        * interval;
                let next = next.min(u128::from(u64::MAX)) as u64;
                registry.timers.insert(
                    tgid,
                    RealTimerState {
                        deadline_nanos: Some(next),
                        interval_nanos: timer.interval_nanos,
                    },
                );
                Some(next)
            };
            if let Some(next) = next_deadline {
                add_deadline(&mut registry, tgid, next);
            }

            expired.push((tgid, SignalInfo::kernel(SIGALRM, 0)));
        }
    }

    publish_next_deadline(&registry);
    expired
}
