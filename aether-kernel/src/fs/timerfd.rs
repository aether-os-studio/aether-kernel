extern crate alloc;

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::sync::{Arc, Weak};
use core::any::Any;
use core::sync::atomic::{AtomicU64, Ordering};

use aether_frame::interrupt::timer;
use aether_frame::libs::spin::SpinLock;
use aether_vfs::{FileOperations, FsError, FsResult, PollEvents, SharedWaitListener, WaitQueue};

use crate::errno::{SysErr, SysResult};
use crate::syscall::abi::{
    CLOCK_BOOTTIME, CLOCK_BOOTTIME_ALARM, CLOCK_MONOTONIC, CLOCK_REALTIME, CLOCK_REALTIME_ALARM,
    LinuxTimespec,
};

pub const TFD_TIMER_ABSTIME: u64 = 0x1;
pub const TFD_TIMER_CANCEL_ON_SET: u64 = 0x2;
pub const TFD_CLOEXEC: u64 = 0o2000000;
pub const TFD_NONBLOCK: u64 = 0o0004000;
pub const TFD_CREATE_FLAGS: u64 = TFD_CLOEXEC | TFD_NONBLOCK;
pub const TFD_SETTIME_FLAGS: u64 = TFD_TIMER_ABSTIME | TFD_TIMER_CANCEL_ON_SET;

static REGISTRY: SpinLock<TimerFdRegistry> = SpinLock::new(TimerFdRegistry::new());
static NEXT_MONOTONIC_DEADLINE_NS: AtomicU64 = AtomicU64::new(u64::MAX);
static NEXT_REALTIME_DEADLINE_NS: AtomicU64 = AtomicU64::new(u64::MAX);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerFdClock {
    Realtime,
    Monotonic,
    Boottime,
    RealtimeAlarm,
    BoottimeAlarm,
}

impl TimerFdClock {
    fn deadline_class(self) -> DeadlineClass {
        match self {
            Self::Realtime | Self::RealtimeAlarm => DeadlineClass::Realtime,
            Self::Monotonic | Self::Boottime | Self::BoottimeAlarm => DeadlineClass::Monotonic,
        }
    }

    fn current_time_ns(self) -> u64 {
        match self {
            Self::Realtime | Self::RealtimeAlarm => {
                let (secs, nanos) = timer::unix_time_nanos();
                (secs as u64)
                    .saturating_mul(1_000_000_000)
                    .saturating_add(nanos)
            }
            Self::Monotonic | Self::Boottime | Self::BoottimeAlarm => timer::nanos_since_boot(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeadlineClass {
    Monotonic,
    Realtime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinuxItimerSpec {
    pub it_interval: LinuxTimespec,
    pub it_value: LinuxTimespec,
}

impl LinuxItimerSpec {
    pub const SIZE: usize = LinuxTimespec::SIZE * 2;

    pub fn read_from(
        ctx: &dyn crate::syscall::KernelSyscallContext,
        address: u64,
    ) -> SysResult<Self> {
        let bytes = ctx.read_user_buffer(address, Self::SIZE)?;
        if bytes.len() != Self::SIZE {
            return Err(SysErr::Fault);
        }

        Ok(Self {
            it_interval: LinuxTimespec {
                tv_sec: i64::from_ne_bytes(bytes[0..8].try_into().map_err(|_| SysErr::Fault)?),
                tv_nsec: i64::from_ne_bytes(bytes[8..16].try_into().map_err(|_| SysErr::Fault)?),
            }
            .validate()?,
            it_value: LinuxTimespec {
                tv_sec: i64::from_ne_bytes(bytes[16..24].try_into().map_err(|_| SysErr::Fault)?),
                tv_nsec: i64::from_ne_bytes(bytes[24..32].try_into().map_err(|_| SysErr::Fault)?),
            }
            .validate()?,
        })
    }

    pub fn write_to(
        self,
        ctx: &mut dyn crate::syscall::KernelSyscallContext,
        address: u64,
    ) -> SysResult<()> {
        let mut bytes = [0u8; Self::SIZE];
        bytes[0..8].copy_from_slice(&self.it_interval.tv_sec.to_ne_bytes());
        bytes[8..16].copy_from_slice(&self.it_interval.tv_nsec.to_ne_bytes());
        bytes[16..24].copy_from_slice(&self.it_value.tv_sec.to_ne_bytes());
        bytes[24..32].copy_from_slice(&self.it_value.tv_nsec.to_ne_bytes());
        ctx.write_user_buffer(address, &bytes)
    }

    pub fn interval_ns(self) -> SysResult<u64> {
        self.it_interval.total_nanos()
    }

    pub fn value_ns(self) -> SysResult<u64> {
        self.it_value.total_nanos()
    }
}

#[derive(Default)]
struct TimerFdRegistry {
    next_id: u64,
    timers: BTreeMap<u64, Weak<TimerFdFile>>,
    monotonic: BTreeMap<u64, BTreeSet<u64>>,
    realtime: BTreeMap<u64, BTreeSet<u64>>,
}

impl TimerFdRegistry {
    const fn new() -> Self {
        Self {
            next_id: 1,
            timers: BTreeMap::new(),
            monotonic: BTreeMap::new(),
            realtime: BTreeMap::new(),
        }
    }

    fn allocate_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        id
    }

    fn deadlines_mut(&mut self, class: DeadlineClass) -> &mut BTreeMap<u64, BTreeSet<u64>> {
        match class {
            DeadlineClass::Monotonic => &mut self.monotonic,
            DeadlineClass::Realtime => &mut self.realtime,
        }
    }

    fn insert_timer(&mut self, id: u64, timer: &Arc<TimerFdFile>) {
        self.timers.insert(id, Arc::downgrade(timer));
    }

    fn remove_timer(&mut self, id: u64) {
        let _ = self.timers.remove(&id);
    }

    fn insert_deadline(&mut self, class: DeadlineClass, deadline_ns: u64, id: u64) {
        self.deadlines_mut(class)
            .entry(deadline_ns)
            .or_default()
            .insert(id);
        self.refresh_next_deadlines();
    }

    fn remove_deadline(&mut self, class: DeadlineClass, deadline_ns: u64, id: u64) {
        let deadlines = self.deadlines_mut(class);
        if let Some(entries) = deadlines.get_mut(&deadline_ns) {
            entries.remove(&id);
            if entries.is_empty() {
                let _ = deadlines.remove(&deadline_ns);
            }
        }
        self.refresh_next_deadlines();
    }

    fn refresh_next_deadlines(&self) {
        let next_monotonic = self
            .monotonic
            .first_key_value()
            .map(|(&deadline, _)| deadline)
            .unwrap_or(u64::MAX);
        let next_realtime = self
            .realtime
            .first_key_value()
            .map(|(&deadline, _)| deadline)
            .unwrap_or(u64::MAX);
        NEXT_MONOTONIC_DEADLINE_NS.store(next_monotonic, Ordering::Release);
        NEXT_REALTIME_DEADLINE_NS.store(next_realtime, Ordering::Release);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TimerFdState {
    interval_ns: u64,
    expires_ns: u64,
    ticks: u64,
    cancel_on_set: bool,
}

pub struct TimerFdFile {
    id: u64,
    clock: TimerFdClock,
    inner: SpinLock<TimerFdState>,
    version: AtomicU64,
    waiters: WaitQueue,
}

impl TimerFdFile {
    pub fn create(clock: TimerFdClock) -> Arc<Self> {
        let mut registry = REGISTRY.lock();
        let file = Arc::new(Self {
            id: registry.allocate_id(),
            clock,
            inner: SpinLock::new(TimerFdState {
                interval_ns: 0,
                expires_ns: 0,
                ticks: 0,
                cancel_on_set: false,
            }),
            version: AtomicU64::new(1),
            waiters: WaitQueue::new(),
        });
        registry.insert_timer(file.id, &file);
        file
    }

    fn bump(&self) {
        let _ = self.version.fetch_add(1, Ordering::AcqRel);
    }

    pub fn version(&self) -> u64 {
        self.version.load(Ordering::Acquire)
    }

    fn refresh_due_locked(
        registry: &mut TimerFdRegistry,
        id: u64,
        clock: TimerFdClock,
        state: &mut TimerFdState,
        now_ns: u64,
    ) -> bool {
        if state.expires_ns == 0 || now_ns < state.expires_ns {
            return false;
        }

        let class = clock.deadline_class();
        registry.remove_deadline(class, state.expires_ns, id);
        if state.interval_ns == 0 {
            state.ticks = state.ticks.saturating_add(1);
            state.expires_ns = 0;
        } else {
            let delta = now_ns.saturating_sub(state.expires_ns);
            let periods = delta / state.interval_ns + 1;
            state.ticks = state.ticks.saturating_add(periods);
            state.expires_ns = state
                .expires_ns
                .saturating_add(periods.saturating_mul(state.interval_ns));
            registry.insert_deadline(class, state.expires_ns, id);
        }
        true
    }

    fn refresh_due(&self) -> bool {
        let now_ns = self.clock.current_time_ns();
        let mut registry = REGISTRY.lock();
        let mut state = self.inner.lock();
        Self::refresh_due_locked(&mut registry, self.id, self.clock, &mut state, now_ns)
    }

    pub fn get_time(&self) -> LinuxItimerSpec {
        let now_ns = self.clock.current_time_ns();
        let mut registry = REGISTRY.lock();
        let mut state = self.inner.lock();
        let _ = Self::refresh_due_locked(&mut registry, self.id, self.clock, &mut state, now_ns);
        let remaining_ns = state.expires_ns.saturating_sub(now_ns);
        LinuxItimerSpec {
            it_interval: ns_to_timespec(state.interval_ns),
            it_value: ns_to_timespec(remaining_ns),
        }
    }

    pub fn set_time(&self, flags: u64, spec: LinuxItimerSpec) -> SysResult<LinuxItimerSpec> {
        let interval_ns = spec.interval_ns()?;
        let value_ns = spec.value_ns()?;
        let absolute = (flags & TFD_TIMER_ABSTIME) != 0;
        let cancel_on_set = (flags & TFD_TIMER_CANCEL_ON_SET) != 0;

        if cancel_on_set
            && (!absolute
                || !matches!(
                    self.clock,
                    TimerFdClock::Realtime | TimerFdClock::RealtimeAlarm
                ))
        {
            return Err(SysErr::Inval);
        }

        let now_ns = self.clock.current_time_ns();
        let class = self.clock.deadline_class();
        let (old_spec, notify_readable) = {
            let mut registry = REGISTRY.lock();
            let mut state = self.inner.lock();
            let _ =
                Self::refresh_due_locked(&mut registry, self.id, self.clock, &mut state, now_ns);

            let old = LinuxItimerSpec {
                it_interval: ns_to_timespec(state.interval_ns),
                it_value: ns_to_timespec(state.expires_ns.saturating_sub(now_ns)),
            };

            if state.expires_ns != 0 {
                registry.remove_deadline(class, state.expires_ns, self.id);
            }

            state.interval_ns = interval_ns;
            state.cancel_on_set = cancel_on_set;
            state.expires_ns = if value_ns == 0 {
                0
            } else if absolute {
                value_ns
            } else {
                now_ns.saturating_add(value_ns)
            };

            if state.expires_ns != 0 {
                if state.expires_ns <= now_ns {
                    let _ = Self::refresh_due_locked(
                        &mut registry,
                        self.id,
                        self.clock,
                        &mut state,
                        now_ns,
                    );
                } else {
                    registry.insert_deadline(class, state.expires_ns, self.id);
                }
            }

            (old, state.ticks != 0)
        };

        self.bump();
        if notify_readable {
            self.waiters.notify(PollEvents::READ);
        }
        Ok(old_spec)
    }
}

impl Drop for TimerFdFile {
    fn drop(&mut self) {
        let mut registry = REGISTRY.lock();
        let state = self.inner.lock();
        if state.expires_ns != 0 {
            registry.remove_deadline(self.clock.deadline_class(), state.expires_ns, self.id);
        }
        drop(state);
        registry.remove_timer(self.id);
        registry.refresh_next_deadlines();
    }
}

impl FileOperations for TimerFdFile {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn read(&self, _offset: usize, buffer: &mut [u8]) -> FsResult<usize> {
        if buffer.len() < core::mem::size_of::<u64>() {
            return Err(FsError::InvalidInput);
        }

        let _ = self.refresh_due();
        let ticks = {
            let mut state = self.inner.lock();
            if state.ticks == 0 {
                return Err(FsError::WouldBlock);
            }
            let ticks = state.ticks;
            state.ticks = 0;
            ticks
        };

        buffer[..8].copy_from_slice(&ticks.to_ne_bytes());
        self.bump();
        Ok(8)
    }

    fn poll(&self, events: PollEvents) -> FsResult<PollEvents> {
        let _ = self.refresh_due();
        let readable = self.inner.lock().ticks != 0;
        let mut ready = PollEvents::empty();
        if readable && events.contains(PollEvents::READ) {
            ready = ready | PollEvents::READ;
        }
        Ok(ready)
    }

    fn wait_token(&self) -> u64 {
        self.version()
    }

    fn register_waiter(
        &self,
        events: PollEvents,
        listener: SharedWaitListener,
    ) -> FsResult<Option<u64>> {
        Ok(Some(self.waiters.register(events, listener)))
    }

    fn unregister_waiter(&self, waiter_id: u64) -> FsResult<()> {
        let _ = self.waiters.unregister(waiter_id);
        Ok(())
    }
}

pub fn deadline_due() -> bool {
    let now_monotonic = timer::nanos_since_boot();
    let monotonic_due = NEXT_MONOTONIC_DEADLINE_NS.load(Ordering::Acquire) <= now_monotonic;
    if monotonic_due {
        return true;
    }

    let (secs, nanos) = timer::unix_time_nanos();
    let now_realtime = (secs as u64)
        .saturating_mul(1_000_000_000)
        .saturating_add(nanos);
    NEXT_REALTIME_DEADLINE_NS.load(Ordering::Acquire) <= now_realtime
}

pub fn next_wakeup_deadline() -> Option<u64> {
    let monotonic = NEXT_MONOTONIC_DEADLINE_NS.load(Ordering::Acquire);
    let realtime = NEXT_REALTIME_DEADLINE_NS.load(Ordering::Acquire);

    let monotonic_deadline = (monotonic != u64::MAX).then_some(monotonic);
    let realtime_deadline = if realtime == u64::MAX {
        None
    } else {
        let now_monotonic = timer::nanos_since_boot();
        let (secs, nanos) = timer::unix_time_nanos();
        let now_realtime = (secs as u64)
            .saturating_mul(1_000_000_000)
            .saturating_add(nanos);
        Some(if realtime <= now_realtime {
            now_monotonic
        } else {
            now_monotonic.saturating_add(realtime.saturating_sub(now_realtime))
        })
    };

    match (monotonic_deadline, realtime_deadline) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

pub fn wake_expired_timers() {
    wake_expired_class(DeadlineClass::Monotonic, timer::nanos_since_boot());
    let (secs, nanos) = timer::unix_time_nanos();
    wake_expired_class(
        DeadlineClass::Realtime,
        (secs as u64)
            .saturating_mul(1_000_000_000)
            .saturating_add(nanos),
    );
}

fn wake_expired_class(class: DeadlineClass, now_ns: u64) {
    loop {
        let ready = {
            let mut registry = REGISTRY.lock();
            let Some((&deadline, _)) = registry.deadlines_mut(class).first_key_value() else {
                registry.refresh_next_deadlines();
                return;
            };
            if deadline > now_ns {
                registry.refresh_next_deadlines();
                return;
            }

            let ids = registry
                .deadlines_mut(class)
                .remove(&deadline)
                .unwrap_or_default()
                .into_iter()
                .collect::<alloc::vec::Vec<_>>();
            registry.refresh_next_deadlines();
            ids.into_iter()
                .filter_map(|id| registry.timers.get(&id).and_then(Weak::upgrade))
                .collect::<alloc::vec::Vec<_>>()
        };

        if ready.is_empty() {
            continue;
        }

        for timerfd in ready {
            let notify_readable = {
                let mut registry = REGISTRY.lock();
                let mut state = timerfd.inner.lock();
                let changed = TimerFdFile::refresh_due_locked(
                    &mut registry,
                    timerfd.id,
                    timerfd.clock,
                    &mut state,
                    now_ns,
                );
                changed && state.ticks != 0
            };
            if notify_readable {
                timerfd.bump();
                timerfd.waiters.notify(PollEvents::READ);
            }
        }
    }
}

pub fn parse_timerfd_clock(clock_id: i32) -> SysResult<TimerFdClock> {
    match clock_id as u64 {
        CLOCK_REALTIME => Ok(TimerFdClock::Realtime),
        CLOCK_MONOTONIC => Ok(TimerFdClock::Monotonic),
        CLOCK_BOOTTIME => Ok(TimerFdClock::Boottime),
        CLOCK_REALTIME_ALARM => {
            // TODO: wake-alarm semantics require RTC/PM integration; treat the clock
            // source as CLOCK_REALTIME until the wakeup path exists.
            Ok(TimerFdClock::RealtimeAlarm)
        }
        CLOCK_BOOTTIME_ALARM => {
            // TODO: wake-alarm semantics require suspend-aware wakeup plumbing.
            Ok(TimerFdClock::BoottimeAlarm)
        }
        _ => Err(SysErr::Inval),
    }
}

fn ns_to_timespec(total_ns: u64) -> LinuxTimespec {
    LinuxTimespec {
        tv_sec: (total_ns / 1_000_000_000) as i64,
        tv_nsec: (total_ns % 1_000_000_000) as i64,
    }
}
