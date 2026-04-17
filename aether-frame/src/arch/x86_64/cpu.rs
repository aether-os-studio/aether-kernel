use crate::boot;

#[must_use]
pub fn current_cpu_index() -> usize {
    let lapic_id = core::arch::x86_64::__cpuid(1).ebx >> 24;
    boot::info()
        .cpus
        .into_iter()
        .position(|cpu| cpu.lapic_id == lapic_id)
        .unwrap_or(0)
}

pub fn wait_for_interrupt() {
    unsafe {
        core::arch::asm!("hlt", options(nomem, nostack, preserves_flags));
    }
}
