#![no_std]
#![allow(unsafe_op_in_unsafe_fn)]

extern crate alloc;
pub extern crate log;

pub mod acpi;
pub mod arch;
pub mod boot;
pub mod bus;
pub mod executor;
pub mod interrupt;
pub mod io;
pub mod libs;
pub mod logger;
pub mod mm;
pub mod preempt;
pub mod process;

#[inline(never)]
pub const fn retain() {}

unsafe extern "C" {
    fn frame_entry() -> !;
}
