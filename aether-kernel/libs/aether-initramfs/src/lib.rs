#![no_std]

extern crate alloc;

use core::str;
use core::sync::atomic::{AtomicU64, Ordering};

use aether_tmpfs as tmpfs;
use aether_vfs::{FsError, NodeRef, Vfs};

#[derive(Debug)]
pub enum InitramfsError {
    InvalidArchive,
    InvalidUtf8,
    UnsupportedEntry,
    FileSystem(FsError),
}

impl From<FsError> for InitramfsError {
    fn from(value: FsError) -> Self {
        Self::FileSystem(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadProgress {
    pub entry_index: u64,
    pub phase: u64,
    pub name_hash: u64,
    pub file_size: u64,
}

static LOAD_ENTRY_INDEX: AtomicU64 = AtomicU64::new(0);
static LOAD_PHASE: AtomicU64 = AtomicU64::new(0);
static LOAD_NAME_HASH: AtomicU64 = AtomicU64::new(0);
static LOAD_FILE_SIZE: AtomicU64 = AtomicU64::new(0);

const PHASE_IDLE: u64 = 0;
const PHASE_ENTRY: u64 = 1;
const PHASE_ENSURE_PARENT: u64 = 2;
const PHASE_BUILD_NODE: u64 = 3;
const PHASE_INSERT_CHILD: u64 = 4;

pub fn load_progress() -> LoadProgress {
    LoadProgress {
        entry_index: LOAD_ENTRY_INDEX.load(Ordering::Acquire),
        phase: LOAD_PHASE.load(Ordering::Acquire),
        name_hash: LOAD_NAME_HASH.load(Ordering::Acquire),
        file_size: LOAD_FILE_SIZE.load(Ordering::Acquire),
    }
}

fn set_load_progress(entry_index: u64, phase: u64, name: &str, file_size: u64) {
    LOAD_ENTRY_INDEX.store(entry_index, Ordering::Release);
    LOAD_PHASE.store(phase, Ordering::Release);
    LOAD_NAME_HASH.store(fnv1a64(name.as_bytes()), Ordering::Release);
    LOAD_FILE_SIZE.store(file_size, Ordering::Release);
}

fn clear_load_progress() {
    LOAD_ENTRY_INDEX.store(0, Ordering::Release);
    LOAD_PHASE.store(PHASE_IDLE, Ordering::Release);
    LOAD_NAME_HASH.store(0, Ordering::Release);
    LOAD_FILE_SIZE.store(0, Ordering::Release);
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

pub fn load_initramfs(bytes: &'static [u8], vfs: &Vfs) -> Result<NodeRef, InitramfsError> {
    let root: NodeRef = tmpfs::directory("/");

    for (entry_index, entry) in cpio_reader::iter_files(bytes).enumerate() {
        let name = entry
            .name()
            .trim_start_matches("./")
            .trim_start_matches('/');
        if name.is_empty() || name == "TRAILER!!!" {
            continue;
        }

        let entry_index = entry_index as u64 + 1;
        let file_size = entry.file().len() as u64;
        set_load_progress(entry_index, PHASE_ENTRY, name, file_size);
        let (parent_path, file_name) = split_parent(name);
        set_load_progress(entry_index, PHASE_ENSURE_PARENT, name, file_size);
        let parent = vfs.ensure_dir_from(root.clone(), parent_path)?;
        set_load_progress(entry_index, PHASE_BUILD_NODE, name, file_size);
        let node = build_node(file_name, &entry)?;
        set_load_progress(entry_index, PHASE_INSERT_CHILD, name, file_size);
        if let Err(error) = parent.insert_child(file_name.into(), node)
            && error != FsError::AlreadyExists
        {
            return Err(error.into());
        }
    }

    clear_load_progress();
    Ok(root)
}

fn build_node(name: &str, entry: &cpio_reader::Entry<'static>) -> Result<NodeRef, InitramfsError> {
    let mode = entry.mode();
    if mode.contains(cpio_reader::Mode::SYMBOLIK_LINK) {
        let target = str::from_utf8(entry.file()).map_err(|_| InitramfsError::InvalidUtf8)?;
        return Ok(tmpfs::symlink_with_mode(name, target, mode.bits()));
    }
    if mode.contains(cpio_reader::Mode::DIRECTORY) {
        return Ok(tmpfs::directory_with_mode(name, mode.bits()));
    }
    if mode.contains(cpio_reader::Mode::REGULAR_FILE) {
        return Ok(tmpfs::borrowed_file_with_mode(
            name,
            entry.file(),
            mode.bits(),
        ));
    }

    Err(InitramfsError::UnsupportedEntry)
}

fn split_parent(path: &str) -> (&str, &str) {
    path.rsplit_once('/').unwrap_or(("", path))
}
