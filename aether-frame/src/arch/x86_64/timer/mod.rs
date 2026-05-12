pub mod hpet;

pub fn stall_nanos(duration_ns: u64) -> Result<(), &'static str> {
    hpet::stall_nanos(duration_ns)
}

#[must_use]
pub const fn supports_deadline_wakeup() -> bool {
    false
}

pub fn publish_wakeup_deadline(_deadline_ns: Option<u64>) {}
