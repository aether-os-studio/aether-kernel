#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

use core::panic::PanicInfo;

use aether_frame::{acpi, boot, interrupt, logger, mm};
use aether_macros::frame_entry;

extern crate aether_frame;

pub mod arch;
pub mod credentials;
pub mod devices;
pub mod errno;
pub mod fs;
pub mod kernfs;
pub mod log_sinks;
pub mod net;
pub mod process;
pub mod procfs;
pub mod rootfs;
pub mod runtime;
pub mod signal;
pub mod syscall;

#[frame_entry]
fn kernel_main() -> ! {
    logger::init();

    log::info!("kernel_main entered");
    log::info!(
        "boot protocol={:?}, cpu_count={}, bsp={:?}",
        boot::info().protocol,
        boot::info().cpus.as_slice().len(),
        boot::info().cpus.bsp().map(|cpu| cpu.lapic_id)
    );

    mm::init().expect("frame allocator initialization failed");

    log::info!("memory management init successfully");
    acpi::init().expect("ACPI initialization failed");
    log::info!(
        "ACPI init successfully, ioapics={}, mcfg_regions={}, hpet={}",
        match acpi::info().interrupt_model() {
            acpi::InterruptModel::Apic(apic) => apic.io_apics.len(),
            acpi::InterruptModel::Unknown => 0,
            _ => 0,
        },
        acpi::info()
            .pci_config_regions()
            .map(|regions| regions.regions.len())
            .unwrap_or(0),
        acpi::info().hpet_info().is_some()
    );

    log::info!("initializing BSP interrupt state");
    interrupt::init_for_cpu(0).expect("interrupt initialization failed");
    #[cfg(target_arch = "x86_64")]
    aether_frame::arch::interrupt::ioapic::init_from_acpi(32)
        .expect("IOAPIC initialization failed");
    log::info!("initializing APIC timer");
    interrupt::timer::init().expect("APIC timer initialization failed");
    let runtime = match runtime::KernelRuntime::bootstrap() {
        Ok(runtime) => runtime,
        Err(error) => panic_runtime(error),
    };
    let runtime = runtime.install();

    let started =
        boot::start_secondary_cpus(secondary_cpu_main).expect("secondary CPU startup failed");
    log::info!("started {} secondary CPUs", started);

    interrupt::enable();

    runtime.run_on_cpu(0)
}

fn panic_runtime(error: runtime::RuntimeInitError) -> ! {
    match error {
        runtime::RuntimeInitError::FileSystem(inner) => {
            panic!("kernel runtime filesystem init failed: {:?}", inner)
        }
        runtime::RuntimeInitError::Framebuffer(inner) => {
            panic!("kernel runtime framebuffer init failed: {:?}", inner)
        }
        runtime::RuntimeInitError::LogWriter(inner) => {
            panic!("kernel runtime log sink init failed: {:?}", inner)
        }
        runtime::RuntimeInitError::Process(inner) => {
            panic!("kernel runtime process init failed: {:?}", inner)
        }
        runtime::RuntimeInitError::Rootfs(inner) => match inner {
            rootfs::RootfsError::FileSystem(fs) => {
                panic!("kernel runtime rootfs filesystem init failed: {:?}", fs)
            }
            rootfs::RootfsError::Initramfs(initramfs) => {
                panic!("kernel runtime initramfs init failed: {:?}", initramfs)
            }
        },
    }
}

fn secondary_cpu_main(cpu_index: usize) -> ! {
    log::info!(
        "secondary cpu {} entered, lapic_id={:?}",
        cpu_index,
        interrupt::current_lapic_id()
    );
    interrupt::init_for_cpu(cpu_index).expect("secondary CPU interrupt initialization failed");
    interrupt::timer::init().expect("secondary CPU APIC timer initialization failed");

    log::info!("secondary cpu {} initialization complete", cpu_index);

    interrupt::enable();
    runtime::KernelRuntime::run_secondary(cpu_index)
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    log::error!("{}", info);
    loop {
        core::hint::spin_loop();
    }
}
