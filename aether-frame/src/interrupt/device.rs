use core::sync::atomic::{AtomicU8, Ordering};

use crate::io::PciConfigSpace;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceInterruptMode {
    Legacy,
    Msi,
    MsiX,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceInterrupt {
    pub vector: u8,
    pub mode: DeviceInterruptMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceInterruptError {
    VectorExhausted,
    Route(&'static str),
}

const DEVICE_VECTOR_START: u8 = 0x90;
const DEVICE_VECTOR_END: u8 = 0xef;
static NEXT_DEVICE_VECTOR: AtomicU8 = AtomicU8::new(DEVICE_VECTOR_START);

pub fn allocate_vector() -> Result<u8, DeviceInterruptError> {
    let mut current = NEXT_DEVICE_VECTOR.load(Ordering::Acquire);
    loop {
        if current > DEVICE_VECTOR_END {
            return Err(DeviceInterruptError::VectorExhausted);
        }
        match NEXT_DEVICE_VECTOR.compare_exchange(
            current,
            current.saturating_add(1),
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => return Ok(current),
            Err(observed) => current = observed,
        }
    }
}

pub fn configure_pci_message_interrupt(
    config: &PciConfigSpace,
    vector: u8,
) -> Result<DeviceInterruptMode, DeviceInterruptError> {
    #[cfg(target_arch = "x86_64")]
    {
        return crate::arch::interrupt::msi::enable_best_available(config, vector)
            .map(|mode| match mode {
                crate::arch::interrupt::msi::PciInterruptMode::Msi => DeviceInterruptMode::Msi,
                crate::arch::interrupt::msi::PciInterruptMode::MsiX => DeviceInterruptMode::MsiX,
            })
            .map_err(DeviceInterruptError::Route);
    }

    #[allow(unreachable_code)]
    Err(DeviceInterruptError::Route(
        "PCI message interrupts are not implemented for this architecture",
    ))
}
