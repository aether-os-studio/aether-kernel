use x86_64::registers::control::Cr2;

use crate::interrupt::{Trap, TrapKind};

pub(crate) fn fault_address_for_trap(trap: Trap) -> u64 {
    if trap.kind() == TrapKind::Exception && trap.vector() == 14 {
        Cr2::read_raw()
    } else {
        0
    }
}
