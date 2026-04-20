mod future;
mod reason;

pub use self::future::RunFuture;
pub use self::reason::{RunReason, RunResult};
pub use crate::arch::process::{
    GeneralRegs, KernelContext, KernelContextEntry, Process, ProcessBuildError, ProcessBuilder,
    UserContext, clear_scheduler_context, initialize_kernel_context,
    initialize_typed_kernel_context, install_scheduler_context, resume_kernel_context,
    run_on_kernel_stack, switch_kernel_context, switch_to_scheduler,
};

pub fn prepare_trap(trap: crate::interrupt::Trap) {
    crate::arch::process::prepare_trap(trap);
}

#[must_use]
pub fn on_trap(
    trap: crate::interrupt::Trap,
    frame: &crate::arch::interrupt::TrapFrame,
) -> Option<RunReason> {
    crate::arch::process::on_trap(trap, frame)
}

pub fn resume_current_user_context() {
    crate::arch::process::resume_current_user_context();
}
