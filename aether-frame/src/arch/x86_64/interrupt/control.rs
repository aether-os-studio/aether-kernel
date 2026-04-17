use core::arch::asm;

#[derive(Clone, Copy)]
pub struct InterruptState {
    enabled: bool,
}

pub fn enable() {
    unsafe {
        asm!("sti", options(nomem, nostack, preserves_flags));
    }
}

#[must_use]
pub fn disable() -> InterruptState {
    let enabled = are_enabled();
    unsafe {
        asm!("cli", options(nomem, nostack, preserves_flags));
    }
    InterruptState { enabled }
}

pub fn restore(state: InterruptState) {
    if state.enabled {
        enable();
    }
}

pub fn are_enabled() -> bool {
    let rflags: u64;
    unsafe {
        asm!("pushfq", "pop {}", out(reg) rflags, options(nomem, preserves_flags));
    }
    (rflags & (1 << 9)) != 0
}
