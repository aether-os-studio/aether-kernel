extern crate alloc;
use aether_vfs::{FsError, FsResult, NodeRef};
use alloc::boxed::Box;
use alloc::sync::Arc;
use ext4plus::prelude::{Ext4, Ext4Read, Ext4Write};

use crate::{AsyncBlockDevice, BlockDeviceFile};

use super::EXT_NAME_LEN;
use super::inode::load_inode_node;
use super::io::{
    ExtBlockReader, ExtBlockWriter, block_on_future, map_ext_error, parse_ext_block_size,
    read_superblock,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExtStat {
    pub block_size: u64,
    pub blocks: u64,
    pub free_blocks: u64,
    pub files: u64,
    pub free_inodes: u64,
    pub name_len: u64,
}

pub struct ExtMount {
    root: NodeRef,
    stat: ExtStat,
}

impl ExtMount {
    pub fn root(&self) -> NodeRef {
        self.root.clone()
    }

    pub const fn stat(&self) -> ExtStat {
        self.stat
    }
}

pub fn mount_from_source(source: &NodeRef, target_name: &str) -> FsResult<ExtMount> {
    let file = source.file().ok_or(FsError::InvalidInput)?;
    let device = file
        .as_any()
        .downcast_ref::<BlockDeviceFile>()
        .map(BlockDeviceFile::device)
        .ok_or(FsError::InvalidInput)?;
    mount_from_block_device(device, target_name)
}

pub fn mount_from_block_device(
    device: Arc<dyn AsyncBlockDevice>,
    target_name: &str,
) -> FsResult<ExtMount> {
    let superblock = block_on_future(read_superblock(device.clone()))?;
    let reader: Box<dyn Ext4Read> = Box::new(ExtBlockReader::new(device.clone()));
    let writer: Box<dyn Ext4Write> = Box::new(ExtBlockWriter::new(device));
    let filesystem =
        block_on_future(Ext4::load_with_writer(reader, Some(writer))).map_err(map_ext_error)?;
    let root = load_inode_node(filesystem.clone(), "/", target_name)?;

    Ok(ExtMount {
        root,
        stat: ExtStat {
            block_size: parse_ext_block_size(&superblock),
            blocks: filesystem.superblock().blocks_count(),
            free_blocks: filesystem.superblock().free_blocks_count(),
            files: parse_u32(&superblock, 0x00) as u64,
            free_inodes: filesystem.superblock().free_inodes_count() as u64,
            name_len: EXT_NAME_LEN,
        },
    })
}

fn parse_u32(bytes: &[u8; super::EXT_SUPERBLOCK_SIZE], offset: usize) -> u32 {
    let mut raw = [0u8; 4];
    raw.copy_from_slice(&bytes[offset..offset + 4]);
    u32::from_le_bytes(raw)
}
