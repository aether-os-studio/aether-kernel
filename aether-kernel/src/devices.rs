extern crate alloc;

use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use aether_device::{DeviceClass, DeviceMetadata, DeviceNode, KernelDevice};
use aether_frame::interrupt::timer;
use aether_vfs::{FileNode, FileOperations, FsResult, IoctlResponse, NodeRef, PollEvents};

pub fn builtin_devices() -> Vec<Arc<dyn KernelDevice>> {
    vec![
        Arc::new(MiscCharDevice::new("null", 1, 3, MiscKind::Null)),
        Arc::new(MiscCharDevice::new("zero", 1, 5, MiscKind::Zero)),
        Arc::new(MiscCharDevice::new("random", 1, 8, MiscKind::Random)),
        Arc::new(MiscCharDevice::new("urandom", 1, 9, MiscKind::Random)),
    ]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MiscKind {
    Null,
    Zero,
    Random,
}

struct MiscCharDevice {
    name: &'static str,
    major: u16,
    minor: u16,
    io: Arc<MiscCharDeviceFile>,
}

impl MiscCharDevice {
    fn new(name: &'static str, major: u16, minor: u16, kind: MiscKind) -> Self {
        Self {
            name,
            major,
            minor,
            io: Arc::new(MiscCharDeviceFile { kind }),
        }
    }
}

impl KernelDevice for MiscCharDevice {
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn metadata(&self) -> DeviceMetadata {
        DeviceMetadata::new(self.name, DeviceClass::Misc, self.major, self.minor)
    }

    fn nodes(&self) -> Vec<DeviceNode> {
        let node: NodeRef = FileNode::new_char_device(
            self.name,
            u32::from(self.major),
            u32::from(self.minor),
            self.io.clone(),
        );
        vec![DeviceNode::new(self.name, node)]
    }
}

struct MiscCharDeviceFile {
    kind: MiscKind,
}

impl MiscCharDeviceFile {
    fn fill_random(buffer: &mut [u8]) {
        static SEED: AtomicU64 = AtomicU64::new(0);

        let mut seed = match SEED.load(Ordering::Relaxed) {
            0 => timer::ticks()
                .wrapping_mul(0x9e37_79b9_7f4a_7c15)
                .wrapping_add(1),
            seed => seed,
        };

        for byte in buffer {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            *byte = seed as u8;
        }

        SEED.store(seed, Ordering::Relaxed);
    }
}

impl FileOperations for MiscCharDeviceFile {
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn read(&self, _offset: usize, buffer: &mut [u8]) -> FsResult<usize> {
        match self.kind {
            MiscKind::Null => Ok(0),
            MiscKind::Zero => {
                buffer.fill(0);
                Ok(buffer.len())
            }
            MiscKind::Random => {
                Self::fill_random(buffer);
                Ok(buffer.len())
            }
        }
    }

    fn write(&self, _offset: usize, buffer: &[u8]) -> FsResult<usize> {
        Ok(buffer.len())
    }

    fn poll(&self, events: PollEvents) -> FsResult<PollEvents> {
        let mut ready = PollEvents::empty();
        if events.contains(PollEvents::READ) {
            ready = ready | PollEvents::READ;
        }
        if events.contains(PollEvents::WRITE) {
            ready = ready | PollEvents::WRITE;
        }
        Ok(ready)
    }

    fn ioctl(&self, _command: u64, _argument: u64) -> FsResult<IoctlResponse> {
        Err(aether_vfs::FsError::Unsupported)
    }
}
