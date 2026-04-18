//! A rust interface to the x2apic interrupt architecture.

#![no_std]
#![allow(internal_features)]
#![allow(clippy::missing_safety_doc)]
#![allow(clippy::unnecessary_cast)]
#![allow(clippy::wrong_self_convention)]
#![feature(ptr_internals)]
#![deny(missing_docs)]

pub mod ioapic;
pub mod lapic;
