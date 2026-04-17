use alloc::vec::Vec;

use acpi::platform::interrupt::{InterruptSourceOverride, Polarity, TriggerMode};
use x2apic::ioapic::{IoApic, IrqFlags, IrqMode, RedirectionTableEntry};

use crate::acpi::InterruptModel;
use crate::io::MmioRegion;
use crate::libs::spin::SpinLock;

struct IoApicController {
    gsi_base: u32,
    irq_count: u32,
    _region: MmioRegion,
    ioapic: SpinLock<IoApic>,
}

static CONTROLLERS: SpinLock<Option<Vec<IoApicController>>> = SpinLock::new(None);

pub fn init_from_acpi(vector_base: u8) -> Result<usize, &'static str> {
    let apic = match crate::acpi::info().interrupt_model() {
        InterruptModel::Apic(apic) => apic,
        _ => return Err("APIC interrupt model is unavailable"),
    };

    let mut controllers = Vec::new();
    for descriptor in &apic.io_apics {
        let region = crate::io::remap_mmio(u64::from(descriptor.address), 0x20)
            .map_err(|_| "failed to remap IOAPIC registers")?;
        let mut ioapic = unsafe { IoApic::new(region.base() as u64) };
        unsafe {
            ioapic.init(vector_base.wrapping_add(descriptor.global_system_interrupt_base as u8));
        }
        let irq_count = unsafe { u32::from(ioapic.max_table_entry()) + 1 };
        controllers.push(IoApicController {
            gsi_base: descriptor.global_system_interrupt_base,
            irq_count,
            _region: region,
            ioapic: SpinLock::new(ioapic),
        });
    }

    let count = controllers.len();
    *CONTROLLERS.lock() = Some(controllers);
    Ok(count)
}

pub fn configure_isa_irq(
    irq: u8,
    vector: u8,
    destination_lapic_id: u8,
) -> Result<(), &'static str> {
    let (gsi, flags) = isa_irq_route(irq)?;
    let controllers = CONTROLLERS.lock();
    let controllers = controllers.as_ref().ok_or("IOAPICs are not initialized")?;
    let controller = controllers
        .iter()
        .find(|controller| {
            gsi >= controller.gsi_base && gsi < controller.gsi_base + controller.irq_count
        })
        .ok_or("no IOAPIC handles the requested GSI")?;
    let pin = (gsi - controller.gsi_base) as u8;

    let mut entry = RedirectionTableEntry::default();
    entry.set_vector(vector);
    entry.set_mode(IrqMode::Fixed);
    entry.set_dest(destination_lapic_id);
    entry.set_flags(flags);

    let mut ioapic = controller.ioapic.lock();
    unsafe {
        ioapic.set_table_entry(pin, entry);
        ioapic.enable_irq(pin);
    }
    Ok(())
}

fn isa_irq_route(irq: u8) -> Result<(u32, IrqFlags), &'static str> {
    let apic = match crate::acpi::info().interrupt_model() {
        InterruptModel::Apic(apic) => apic,
        _ => return Err("APIC interrupt model is unavailable"),
    };

    if let Some(override_entry) = apic
        .interrupt_source_overrides
        .iter()
        .find(|entry| entry.isa_source == irq)
    {
        return Ok((
            override_entry.global_system_interrupt,
            flags_from_override(override_entry),
        ));
    }

    Ok((u32::from(irq), IrqFlags::empty()))
}

fn flags_from_override(override_entry: &InterruptSourceOverride) -> IrqFlags {
    let mut flags = IrqFlags::empty();
    if override_entry.polarity == Polarity::ActiveLow {
        flags |= IrqFlags::LOW_ACTIVE;
    }
    if override_entry.trigger_mode == TriggerMode::Level {
        flags |= IrqFlags::LEVEL_TRIGGERED;
    }
    flags
}
