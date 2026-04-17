extern crate alloc;

use alloc::sync::Arc;

use aether_frame::libs::spin::SpinLock;

use crate::DentryRef;

pub type SuperBlockRef = Arc<SuperBlock>;

pub struct SuperBlock {
    fs_type: &'static str,
    device_id: u64,
    root: SpinLock<Option<DentryRef>>,
}

impl SuperBlock {
    pub const fn new(fs_type: &'static str, device_id: u64) -> Self {
        Self {
            fs_type,
            device_id,
            root: SpinLock::new(None),
        }
    }

    pub fn shared(fs_type: &'static str, device_id: u64) -> SuperBlockRef {
        Arc::new(Self::new(fs_type, device_id))
    }

    pub fn fs_type(&self) -> &'static str {
        self.fs_type
    }

    pub fn device_id(&self) -> u64 {
        self.device_id
    }

    pub fn root(&self) -> Option<DentryRef> {
        self.root.lock().clone()
    }

    pub fn set_root(&self, root: DentryRef) {
        *self.root.lock() = Some(root);
    }
}
