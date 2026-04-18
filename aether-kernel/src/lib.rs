#![no_std]
#![forbid(unsafe_code)]
#![feature(alloc_error_handler)]

extern crate alloc;

use core::alloc::Layout;
use core::panic::PanicInfo;

use aether_frame::{boot, interrupt};
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
pub mod processor;
pub mod procfs;
pub mod rootfs;
pub mod runtime;
pub mod signal;
pub mod syscall;

#[frame_entry(secondary = "secondary_cpu_main")]
fn kernel_main() -> ! {
    log::info!("kernel_main entered");
    log::info!(
        "boot protocol={:?}, cpu_count={}, bsp={:?}",
        boot::info().protocol,
        boot::info().cpus.as_slice().len(),
        boot::info().cpus.bsp().map(|cpu| cpu.lapic_id)
    );
    let runtime = match runtime::KernelRuntime::bootstrap() {
        Ok(runtime) => runtime,
        Err(error) => panic_runtime(error),
    };
    let runtime = runtime.install();

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
        runtime::RuntimeInitError::Processor(inner) => {
            panic!("kernel runtime processor init failed: {}", inner)
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
    runtime::KernelRuntime::run_secondary(cpu_index)
}

#[alloc_error_handler]
fn alloc_error(layout: Layout) -> ! {
    panic!(
        "allocation failed: size={} align={}",
        layout.size(),
        layout.align(),
    );
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    log::error!("{}", info);
    loop {
        core::hint::spin_loop();
    }
}
