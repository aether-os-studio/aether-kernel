pub mod apic;
mod control;
pub mod gdt;
mod idt;
pub mod ioapic;
pub mod msi;
pub mod pic;
mod trap;

use crate::arch::fpu;
use crate::preempt;

pub use self::control::{InterruptState, are_enabled, disable, enable, restore};
pub use self::trap::TrapFrame;

pub fn init_for_cpu(cpu_index: usize) -> Result<(), &'static str> {
    preempt::init_for_cpu(cpu_index)?;
    fpu::init_for_cpu(cpu_index)?;
    gdt::init(cpu_index)?;
    idt::init();
    trap::init_syscall(cpu_index)?;
    apic::init(cpu_index)
}

pub(crate) fn init_preempt_ipi() -> Result<(), &'static str> {
    apic::init_preempt_ipi()
}

pub(crate) fn kick_cpu(cpu_index: usize) -> Result<(), &'static str> {
    apic::kick_cpu(cpu_index)
}

pub(crate) fn install_process_kernel_stack(stack_top: u64) {
    gdt::set_kernel_stack(stack_top);
}

pub(crate) fn install_process_syscall_stack(stack_top: u64) {
    trap::set_syscall_kernel_stack(stack_top);
}

pub(crate) fn install_user_entry_context(
    current_run: *const crate::arch::process::CurrentRun,
    user_context: *mut crate::arch::process::UserContext,
) {
    trap::set_user_entry_context(current_run, user_context);
}

pub(crate) fn finish_interrupt(vector: u8) {
    if apic::vector_requires_eoi(vector) {
        apic::end_of_interrupt();
    }
}
