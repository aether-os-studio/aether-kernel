extern crate alloc;

use aether_vfs::{FsError, FsResult};
use alloc::boxed::Box;
use async_trait::async_trait;
use core::cmp::min;
use core::error::Error;
use core::fmt::{self, Display, Formatter};
use core::future::Future;
use ext4plus::prelude::{Ext4Error, Ext4Read, Ext4Write};

use crate::AsyncBlockDevice;

use super::{EXT_SUPERBLOCK_OFFSET, EXT_SUPERBLOCK_SIZE};

#[derive(Clone)]
pub(crate) struct ExtBlockReader {
    device: alloc::sync::Arc<dyn AsyncBlockDevice>,
}

impl ExtBlockReader {
    pub(crate) fn new(device: alloc::sync::Arc<dyn AsyncBlockDevice>) -> Self {
        Self { device }
    }
}

#[derive(Clone)]
pub(crate) struct ExtBlockWriter {
    device: alloc::sync::Arc<dyn AsyncBlockDevice>,
}

impl ExtBlockWriter {
    pub(crate) fn new(device: alloc::sync::Arc<dyn AsyncBlockDevice>) -> Self {
        Self { device }
    }
}

#[async_trait]
impl Ext4Read for ExtBlockReader {
    async fn read(
        &self,
        start_byte: u64,
        dst: &mut [u8],
    ) -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
        read_exact(self.device.as_ref(), start_byte, dst)
            .await
            .map_err(|error| Box::new(ExtBlockIoError(error)) as Box<dyn Error + Send + Sync>)
    }
}

#[async_trait]
impl Ext4Write for ExtBlockWriter {
    async fn write(
        &self,
        start_byte: u64,
        src: &[u8],
    ) -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
        write_exact(self.device.as_ref(), start_byte, src)
            .await
            .map_err(|error| Box::new(ExtBlockIoError(error)) as Box<dyn Error + Send + Sync>)
    }
}

#[derive(Debug, Clone, Copy)]
struct ExtBlockIoError(FsError);

impl Display for ExtBlockIoError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "block io error: {:?}", self.0)
    }
}

impl Error for ExtBlockIoError {}

pub(crate) async fn read_superblock(
    device: alloc::sync::Arc<dyn AsyncBlockDevice>,
) -> FsResult<[u8; EXT_SUPERBLOCK_SIZE]> {
    let mut bytes = [0u8; EXT_SUPERBLOCK_SIZE];
    read_exact(device.as_ref(), EXT_SUPERBLOCK_OFFSET, &mut bytes).await?;
    Ok(bytes)
}

pub(crate) async fn read_exact(
    device: &dyn AsyncBlockDevice,
    offset: u64,
    buffer: &mut [u8],
) -> FsResult<()> {
    let geometry = device.geometry();
    if !geometry.is_valid() {
        return Err(FsError::InvalidInput);
    }

    let end = offset
        .checked_add(buffer.len() as u64)
        .ok_or(FsError::InvalidInput)?;
    if end > device.size_bytes() {
        return Err(FsError::InvalidInput);
    }

    let mut remaining = buffer.len();
    let mut copied = 0usize;
    let mut block = offset / geometry.block_size as u64;
    let mut block_offset = (offset % geometry.block_size as u64) as usize;
    let mut scratch = None::<crate::TransferBufferLease>;

    while remaining > 0 {
        if block_offset == 0 {
            let direct_bytes = remaining - (remaining % geometry.block_size);
            if direct_bytes != 0 {
                let available = device
                    .read_blocks(block, &mut buffer[copied..copied + direct_bytes])
                    .await?;
                if available == 0 {
                    return Err(FsError::InvalidInput);
                }

                copied += available;
                remaining -= available;
                block = block.saturating_add((available / geometry.block_size) as u64);
                if available < direct_bytes {
                    return Err(FsError::InvalidInput);
                }
                continue;
            }
        }

        let scratch =
            scratch.get_or_insert_with(|| device.acquire_transfer_buffer(geometry.block_size));
        let available = device
            .read_blocks(block, scratch.as_mut_slice(geometry.block_size))
            .await?;
        if available <= block_offset {
            return Err(FsError::InvalidInput);
        }

        let chunk = min(remaining, available - block_offset);
        let scratch = scratch.as_mut_slice(available);
        buffer[copied..copied + chunk]
            .copy_from_slice(&scratch[block_offset..block_offset + chunk]);
        copied += chunk;
        remaining -= chunk;
        block = block.saturating_add(1);
        block_offset = 0;
    }

    Ok(())
}

pub(crate) async fn write_exact(
    device: &dyn AsyncBlockDevice,
    offset: u64,
    buffer: &[u8],
) -> FsResult<()> {
    let geometry = device.geometry();
    if !geometry.is_valid() {
        return Err(FsError::InvalidInput);
    }

    let end = offset
        .checked_add(buffer.len() as u64)
        .ok_or(FsError::InvalidInput)?;
    if end > device.size_bytes() {
        return Err(FsError::InvalidInput);
    }

    let mut remaining = buffer.len();
    let mut copied = 0usize;
    let mut block = offset / geometry.block_size as u64;
    let mut block_offset = (offset % geometry.block_size as u64) as usize;
    let mut scratch = None::<crate::TransferBufferLease>;

    while remaining > 0 {
        if block_offset == 0 {
            let direct_bytes = remaining - (remaining % geometry.block_size);
            if direct_bytes != 0 {
                let written = device
                    .write_blocks(block, &buffer[copied..copied + direct_bytes])
                    .await?;
                if written == 0 {
                    return Err(FsError::InvalidInput);
                }

                copied += written;
                remaining -= written;
                block = block.saturating_add((written / geometry.block_size) as u64);
                if written < direct_bytes {
                    return Err(FsError::InvalidInput);
                }
                continue;
            }
        }

        let scratch =
            scratch.get_or_insert_with(|| device.acquire_transfer_buffer(geometry.block_size));
        let prepared = device
            .read_blocks(block, scratch.as_mut_slice(geometry.block_size))
            .await?;
        if prepared <= block_offset {
            return Err(FsError::InvalidInput);
        }

        let chunk = min(remaining, prepared - block_offset);
        let scratch = scratch.as_mut_slice(prepared);
        scratch[block_offset..block_offset + chunk]
            .copy_from_slice(&buffer[copied..copied + chunk]);
        let written = device.write_blocks(block, &scratch[..prepared]).await?;
        if written < prepared {
            return Err(FsError::InvalidInput);
        }

        copied += chunk;
        remaining -= chunk;
        block = block.saturating_add(1);
        block_offset = 0;
    }

    Ok(())
}

pub(crate) fn parse_ext_block_size(superblock: &[u8; EXT_SUPERBLOCK_SIZE]) -> u64 {
    1024u64 << parse_u32(superblock, 0x18)
}

fn parse_u32(bytes: &[u8; EXT_SUPERBLOCK_SIZE], offset: usize) -> u32 {
    let mut raw = [0u8; 4];
    raw.copy_from_slice(&bytes[offset..offset + 4]);
    u32::from_le_bytes(raw)
}

pub(crate) fn map_ext_error(error: Ext4Error) -> FsError {
    match error {
        Ext4Error::NotFound => FsError::NotFound,
        Ext4Error::NotADirectory => FsError::NotDirectory,
        Ext4Error::IsADirectory => FsError::NotFile,
        Ext4Error::AlreadyExists => FsError::AlreadyExists,
        Ext4Error::IsASpecialFile
        | Ext4Error::Encrypted
        | Ext4Error::Readonly
        | Ext4Error::NoSpace => FsError::Unsupported,
        Ext4Error::Io(_)
        | Ext4Error::Incompatible(_)
        | Ext4Error::Corrupt(_)
        | Ext4Error::NotAbsolute
        | Ext4Error::NotASymlink
        | Ext4Error::FileTooLarge
        | Ext4Error::NotUtf8
        | Ext4Error::MalformedPath
        | Ext4Error::PathTooLong
        | Ext4Error::TooManySymlinks
        | Ext4Error::DotEntry
        | _ => FsError::InvalidInput,
    }
}

pub(crate) fn block_on_future<F>(future: F) -> F::Output
where
    F: Future,
{
    crate::block_on_future(future)
}
