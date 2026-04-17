#![no_std]

extern crate alloc;

use core::str;

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

pub fn load_initramfs(bytes: &[u8], vfs: &Vfs) -> Result<NodeRef, InitramfsError> {
    let root: NodeRef = tmpfs::directory("/");

    for entry in cpio_reader::iter_files(bytes) {
        let name = entry
            .name()
            .trim_start_matches("./")
            .trim_start_matches('/');
        if name.is_empty() || name == "TRAILER!!!" {
            continue;
        }

        let (parent_path, file_name) = split_parent(name);
        let parent = vfs.ensure_dir_from(root.clone(), parent_path)?;
        let node = build_node(file_name, &entry)?;
        if let Err(error) = parent.insert_child(file_name.into(), node)
            && error != FsError::AlreadyExists
        {
            return Err(error.into());
        }
    }

    Ok(root)
}

fn build_node(name: &str, entry: &cpio_reader::Entry<'_>) -> Result<NodeRef, InitramfsError> {
    let mode = entry.mode();
    if mode.contains(cpio_reader::Mode::SYMBOLIK_LINK) {
        let target = str::from_utf8(entry.file()).map_err(|_| InitramfsError::InvalidUtf8)?;
        return Ok(tmpfs::symlink_with_mode(name, target, mode.bits() as u32));
    }
    if mode.contains(cpio_reader::Mode::DIRECTORY) {
        return Ok(tmpfs::directory_with_mode(name, mode.bits() as u32));
    }
    if mode.contains(cpio_reader::Mode::REGULAR_FILE) {
        return Ok(tmpfs::file_with_mode(
            name,
            entry.file(),
            mode.bits() as u32,
        ));
    }

    Err(InitramfsError::UnsupportedEntry)
}

fn split_parent(path: &str) -> (&str, &str) {
    path.rsplit_once('/').unwrap_or(("", path))
}
