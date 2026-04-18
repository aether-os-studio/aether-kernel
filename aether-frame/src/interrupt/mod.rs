pub mod device;
mod dispatch;
pub mod softirq;
pub mod timer;
pub(crate) mod trap;

pub use self::dispatch::{HandlerRegistrationError, TrapHandler, dispatch_trap, register_handler};
pub use self::trap::{PrivilegeLevel, SYSCALL_TRAP_VECTOR, Trap, TrapKind};
pub use crate::arch::interrupt::TrapFrame;

pub fn init_for_cpu(cpu_index: usize) -> Result<(), &'static str> {
    crate::arch::interrupt::init_for_cpu(cpu_index)
}

pub(crate) fn init_preempt_ipi() -> Result<(), &'static str> {
    crate::arch::interrupt::init_preempt_ipi()
}

pub(crate) fn kick_cpu(cpu_index: usize) -> Result<(), &'static str> {
    crate::arch::interrupt::kick_cpu(cpu_index)
}

#[must_use]
pub fn current_lapic_id() -> Option<u32> {
    crate::arch::interrupt::apic::current_lapic_id()
}

pub fn enable() {
    crate::arch::interrupt::enable();
}

#[must_use]
pub fn disable() -> crate::arch::interrupt::InterruptState {
    crate::arch::interrupt::disable()
}

pub fn restore(state: crate::arch::interrupt::InterruptState) {
    crate::arch::interrupt::restore(state);
}

#[must_use]
pub fn are_enabled() -> bool {
    crate::arch::interrupt::are_enabled()
}
