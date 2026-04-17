use crate::arch::interrupt::TrapFrame;

use super::{Trap, TrapKind};

pub type TrapHandler = fn(Trap, &mut TrapFrame);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandlerRegistrationError {
    ReservedVector,
}

static mut HANDLERS: [Option<TrapHandler>; 256] = [None; 256];

pub fn register_handler(vector: u8, handler: TrapHandler) -> Result<(), HandlerRegistrationError> {
    if vector < 32 && vector != super::trap::SYSCALL_TRAP_VECTOR {
        return Err(HandlerRegistrationError::ReservedVector);
    }

    unsafe {
        HANDLERS[vector as usize] = Some(handler);
    }
    Ok(())
}

pub fn dispatch_trap(trap: Trap, frame: &mut TrapFrame) {
    let handler = unsafe { HANDLERS[trap.vector() as usize] };
    if let Some(handler) = handler {
        handler(trap, frame);
        return;
    }

    if trap.kind() == TrapKind::Exception && !frame.from_user() {
        log::error!(
            "unhandled kernel exception: vector={}, error_code={:#x}, rip={:#x}",
            trap.vector(),
            trap.error_code(),
            frame.rip()
        );
    }
}
