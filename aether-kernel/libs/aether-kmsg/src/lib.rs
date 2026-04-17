#![no_std]

extern crate alloc;

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::fmt::{self, Write};

use aether_frame::interrupt::timer;
use aether_frame::libs::spin::SpinLock;
use aether_vfs::{FileOperations, FsResult};
use log::Level;

const DEFAULT_KMSG_RECORDS: usize = 256;

#[derive(Debug, Clone)]
pub struct KmsgRecord {
    pub sequence: u64,
    pub level: Level,
    pub timestamp_ticks: u64,
    pub text: String,
}

struct KmsgState {
    next_sequence: u64,
    records: VecDeque<KmsgRecord>,
}

pub struct KmsgBuffer {
    capacity: usize,
    state: SpinLock<KmsgState>,
}

impl KmsgBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            state: SpinLock::new(KmsgState {
                next_sequence: 0,
                records: VecDeque::new(),
            }),
        }
    }

    pub fn push(&self, level: Level, args: fmt::Arguments<'_>) {
        let mut text = String::new();
        let _ = text.write_fmt(args);

        let mut state = self.state.lock();
        let record = KmsgRecord {
            sequence: state.next_sequence,
            level,
            timestamp_ticks: timer::ticks(),
            text,
        };
        state.next_sequence = state.next_sequence.wrapping_add(1);
        if state.records.len() == self.capacity {
            state.records.pop_front();
        }
        state.records.push_back(record);
    }

    pub fn snapshot(&self) -> Vec<KmsgRecord> {
        self.state.lock().records.iter().cloned().collect()
    }

    pub fn file(self: &Arc<Self>) -> Arc<KmsgFile> {
        Arc::new(KmsgFile {
            buffer: self.clone(),
        })
    }

    fn render(&self) -> String {
        let mut content = String::new();
        for record in self.snapshot() {
            let _ = writeln!(
                content,
                "<{}>,{},{};{}",
                level_to_priority(record.level),
                record.sequence,
                record.timestamp_ticks,
                record.text
            );
        }
        content
    }
}

impl Default for KmsgBuffer {
    fn default() -> Self {
        Self::new(DEFAULT_KMSG_RECORDS)
    }
}

pub struct KmsgFile {
    buffer: Arc<KmsgBuffer>,
}

impl FileOperations for KmsgFile {
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn read(&self, offset: usize, buffer: &mut [u8]) -> FsResult<usize> {
        let content = self.buffer.render();
        let bytes = content.as_bytes();
        if offset >= bytes.len() {
            return Ok(0);
        }

        let count = core::cmp::min(buffer.len(), bytes.len() - offset);
        buffer[..count].copy_from_slice(&bytes[offset..offset + count]);
        Ok(count)
    }

    fn size(&self) -> usize {
        self.buffer.render().len()
    }
}

fn level_to_priority(level: Level) -> u8 {
    match level {
        Level::Error => 3,
        Level::Warn => 4,
        Level::Info => 6,
        Level::Debug | Level::Trace => 7,
    }
}
