use crate::arch::process::UserContext;
use crate::interrupt::{Trap, TrapKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunReason {
    Syscall,
    Interrupt {
        vector: u8,
    },
    Exception {
        vector: u8,
        error_code: u64,
        fault_address: u64,
    },
}

impl RunReason {
    pub(crate) const fn from_trap(trap: Trap, fault_address: u64) -> Self {
        match trap.kind() {
            TrapKind::Syscall => Self::Syscall,
            TrapKind::Interrupt => Self::Interrupt {
                vector: trap.vector(),
            },
            TrapKind::Exception => Self::Exception {
                vector: trap.vector(),
                error_code: trap.error_code(),
                fault_address,
            },
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RunResult {
    pub reason: RunReason,
    pub context: UserContext,
}
