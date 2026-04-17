extern crate alloc;

use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, Ordering};

use aether_frame::libs::spin::SpinLock;
use aether_frame::logger::{LogMessage, RegisterWriterError, register_writer};
use aether_kmsg::KmsgBuffer;
use aether_terminal::FramebufferConsole;

static WRITERS_READY: AtomicBool = AtomicBool::new(false);
static KMSG_SINK: SpinLock<Option<Arc<KmsgBuffer>>> = SpinLock::new(None);
static TERMINAL_SINK: SpinLock<Option<Arc<FramebufferConsole>>> = SpinLock::new(None);

pub fn init() -> Result<(), RegisterWriterError> {
    if WRITERS_READY.swap(true, Ordering::AcqRel) {
        return Ok(());
    }

    register_writer(kmsg_writer)?;
    Ok(())
}

pub fn install_kmsg(buffer: Arc<KmsgBuffer>) {
    *KMSG_SINK.lock() = Some(buffer);
}

pub fn install_terminal(console: Arc<FramebufferConsole>) {
    *TERMINAL_SINK.lock() = Some(console);
}

pub fn kmsg_writer(message: &LogMessage<'_>) {
    let sink = KMSG_SINK.lock().clone();
    if let Some(buffer) = sink {
        buffer.push(
            message.level,
            format_args!(
                "[{}:{}] [{}] {}",
                message.file, message.line, message.level, message.args
            ),
        );
    }
}
