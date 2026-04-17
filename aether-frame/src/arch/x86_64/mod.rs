pub mod cpu;
pub mod fpu;
pub mod interrupt;
pub mod io;
pub mod mm;
pub mod process;
pub mod serial;
pub mod timer;

use core::arch::asm;

use crate::boot;

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    if !boot::limine_base_revision_supported() {
        crate::serial_println!("limine base revision 6 is not supported");
        halt_forever();
    }

    unsafe {
        if !boot::install_limine() {
            crate::serial_println!("limine boot information is incomplete");
            halt_forever();
        }
    }

    crate::startup::boot_and_enter_kernel();
}

fn halt_forever() -> ! {
    loop {
        unsafe {
            asm!("hlt", options(nomem, nostack, preserves_flags));
        }
    }
}
