#![allow(dead_code)]

extern crate alloc;

mod dentry;
mod file;
mod inode;
mod io;
mod mount;

pub use self::mount::{ExtMount, ExtStat, mount_from_block_device, mount_from_source};

pub const EXT_SUPER_MAGIC: u64 = 0x0000_ef53;
const EXT_NAME_LEN: u64 = 255;
const EXT_SUPERBLOCK_OFFSET: u64 = 1024;
const EXT_SUPERBLOCK_SIZE: usize = 1024;
