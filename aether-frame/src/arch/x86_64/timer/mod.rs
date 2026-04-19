pub mod hpet;

pub fn stall_nanos(duration_ns: u64) -> Result<(), &'static str> {
    hpet::stall_nanos(duration_ns)
}
