use core::time::Duration;

pub const NANOS_PER_SECOND: u64 = 1_000_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct MonotonicInstant {
    nanos: u64,
}

impl MonotonicInstant {
    pub const fn from_nanos(nanos: u64) -> Self {
        Self { nanos }
    }

    pub fn now() -> Self {
        monotonic_now()
    }

    pub const fn as_nanos(self) -> u64 {
        self.nanos
    }

    pub fn as_duration(self) -> Duration {
        Duration::from_nanos(self.nanos)
    }

    pub fn saturating_add_nanos(self, nanos: u64) -> Self {
        Self::from_nanos(self.nanos.saturating_add(nanos))
    }

    pub fn saturating_duration_since(self, earlier: Self) -> Duration {
        Duration::from_nanos(self.nanos.saturating_sub(earlier.nanos))
    }

    pub fn saturating_nanos_since(self, earlier: Self) -> u64 {
        self.nanos.saturating_sub(earlier.nanos)
    }

    pub fn is_reached(self) -> bool {
        monotonic_now() >= self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RealtimeInstant {
    seconds: i64,
    nanoseconds: u32,
}

impl RealtimeInstant {
    pub const fn new(seconds: i64, nanoseconds: u32) -> Self {
        Self {
            seconds,
            nanoseconds,
        }
    }

    pub fn now() -> Self {
        realtime_now()
    }

    pub const fn seconds(self) -> i64 {
        self.seconds
    }

    pub const fn nanoseconds(self) -> u32 {
        self.nanoseconds
    }

    pub const fn split(self) -> (i64, u32) {
        (self.seconds, self.nanoseconds)
    }

    pub fn total_nanos(self) -> Option<u64> {
        (self.seconds >= 0).then(|| {
            (self.seconds as u64)
                .saturating_mul(NANOS_PER_SECOND)
                .saturating_add(self.nanoseconds as u64)
        })
    }
}

pub fn monotonic_now() -> MonotonicInstant {
    MonotonicInstant::from_nanos(monotonic_nanos())
}

pub fn monotonic_nanos() -> u64 {
    crate::interrupt::timer::nanos_since_boot()
}

pub fn realtime_now() -> RealtimeInstant {
    let (seconds, nanoseconds) = crate::interrupt::timer::unix_time_nanos();
    RealtimeInstant::new(seconds, nanoseconds as u32)
}

pub fn realtime_nanos() -> (i64, u32) {
    realtime_now().split()
}

pub fn realtime_seconds() -> i64 {
    realtime_now().seconds()
}

pub fn spin_delay(duration: Duration) -> Result<(), &'static str> {
    let nanos = duration.as_nanos().min(u128::from(u64::MAX)) as u64;
    spin_delay_nanos(nanos)
}

pub fn spin_delay_nanos(duration_ns: u64) -> Result<(), &'static str> {
    crate::arch::timer::stall_nanos(duration_ns)
}
