use core::fmt;

use log::{Level, LevelFilter, Log, Metadata, Record, set_logger, set_max_level};

use crate::libs::spin::{LocalIrqDisabled, SpinLock};

const MAX_LOG_WRITERS: usize = 8;

#[derive(Clone, Copy)]
pub struct LogMessage<'a> {
    pub level: Level,
    pub file: &'a str,
    pub line: u32,
    pub args: fmt::Arguments<'a>,
}

pub type LogWriter = fn(&LogMessage<'_>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegisterWriterError {
    Duplicate,
    CapacityExceeded,
}

static LOG_WRITERS: SpinLock<[Option<LogWriter>; MAX_LOG_WRITERS], LocalIrqDisabled> =
    SpinLock::new([None; MAX_LOG_WRITERS]);

pub struct KernelLogger;

impl KernelLogger {
    fn log_message(&self, record: &Record) {
        let message = LogMessage {
            level: record.level(),
            file: record.file().unwrap_or("<unknown>"),
            line: record.line().unwrap_or(0),
            args: *record.args(),
        };
        let color = match record.level() {
            Level::Error => "31",
            Level::Warn => "33",
            Level::Info => "32",
            Level::Debug => "34",
            Level::Trace => "35",
        };

        crate::serial_println!(
            "[{}:{}] [{}] {}",
            message.file,
            message.line,
            format_args!("\x1b[{}m{}\x1b[0m", color, record.level()),
            message.args,
        );

        for writer in snapshot_writers().into_iter().flatten() {
            writer(&message);
        }
    }
}

impl Log for KernelLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= log::max_level()
    }

    fn flush(&self) {}

    fn log(&self, record: &Record) {
        self.log_message(record);
    }
}

pub fn init() {
    let _ = set_logger(&KernelLogger);
    set_max_level(LevelFilter::Debug);
}

pub fn register_writer(writer: LogWriter) -> Result<(), RegisterWriterError> {
    let mut writers = LOG_WRITERS.lock();

    if writers
        .iter()
        .flatten()
        .any(|existing| *existing as usize == writer as usize)
    {
        return Err(RegisterWriterError::Duplicate);
    }

    let slot = writers
        .iter_mut()
        .find(|slot| slot.is_none())
        .ok_or(RegisterWriterError::CapacityExceeded)?;
    *slot = Some(writer);
    Ok(())
}

fn snapshot_writers() -> [Option<LogWriter>; MAX_LOG_WRITERS] {
    *LOG_WRITERS.lock()
}
