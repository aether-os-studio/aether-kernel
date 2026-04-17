use crate::{acpi, boot, interrupt, logger, mm};

unsafe extern "C" {
    fn kernel_frame_main() -> !;
    fn kernel_frame_secondary_main(cpu_index: usize) -> !;
}

pub fn boot_and_enter_kernel() -> ! {
    logger::init();
    mm::init().expect("frame allocator initialization failed");
    acpi::init().expect("ACPI initialization failed");
    interrupt::init_for_cpu(0).expect("interrupt initialization failed");
    #[cfg(target_arch = "x86_64")]
    {
        crate::arch::interrupt::pic::disable_legacy_pic();
        crate::arch::interrupt::ioapic::init_from_acpi(32).expect("IOAPIC initialization failed");
    }
    interrupt::timer::init().expect("APIC timer initialization failed");

    let started =
        boot::start_secondary_cpus(secondary_cpu_entry).expect("secondary CPU startup failed");
    log::info!("frame: started {} secondary CPUs", started);

    interrupt::enable();

    unsafe { kernel_frame_main() }
}

fn secondary_cpu_entry(cpu_index: usize) -> ! {
    interrupt::init_for_cpu(cpu_index).expect("secondary CPU interrupt initialization failed");
    interrupt::timer::init().expect("secondary CPU APIC timer initialization failed");
    interrupt::enable();
    unsafe { kernel_frame_secondary_main(cpu_index) }
}
