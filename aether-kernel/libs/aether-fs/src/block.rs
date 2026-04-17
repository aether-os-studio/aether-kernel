extern crate alloc;

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec;
use core::future::Future;
use core::pin::Pin;

use aether_vfs::{FileOperations, FsError, FsResult};

pub type BlockFuture<'a, T> = Pin<Box<dyn Future<Output = FsResult<T>> + Send + 'a>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockGeometry {
    pub block_size: usize,
    pub block_count: u64,
}

impl BlockGeometry {
    pub const fn new(block_size: usize, block_count: u64) -> Self {
        Self {
            block_size,
            block_count,
        }
    }

    pub const fn size_bytes(self) -> u64 {
        self.block_size as u64 * self.block_count
    }

    pub const fn is_valid(self) -> bool {
        self.block_size != 0
    }
}

pub trait AsyncBlockDevice: Send + Sync {
    fn geometry(&self) -> BlockGeometry;

    fn read_blocks<'a>(&'a self, block: u64, buffer: &'a mut [u8]) -> BlockFuture<'a, usize>;

    fn write_blocks<'a>(&'a self, _block: u64, _buffer: &'a [u8]) -> BlockFuture<'a, usize> {
        Box::pin(async { Err(FsError::Unsupported) })
    }

    fn flush<'a>(&'a self) -> BlockFuture<'a, ()> {
        Box::pin(async { Ok(()) })
    }

    fn size_bytes(&self) -> u64 {
        self.geometry().size_bytes()
    }
}

pub trait SyncBlockDevice: Send + Sync {
    fn geometry(&self) -> BlockGeometry;

    fn read_blocks(&self, block: u64, buffer: &mut [u8]) -> FsResult<usize>;

    fn write_blocks(&self, _block: u64, _buffer: &[u8]) -> FsResult<usize> {
        Err(FsError::Unsupported)
    }

    fn flush(&self) -> FsResult<()> {
        Ok(())
    }
}

#[derive(Clone)]
pub struct SyncToAsyncBlockDevice<D> {
    inner: D,
}

impl<D> SyncToAsyncBlockDevice<D> {
    pub const fn new(inner: D) -> Self {
        Self { inner }
    }

    pub fn into_inner(self) -> D {
        self.inner
    }
}

impl<D: SyncBlockDevice> AsyncBlockDevice for SyncToAsyncBlockDevice<D> {
    fn geometry(&self) -> BlockGeometry {
        self.inner.geometry()
    }

    fn read_blocks<'a>(&'a self, block: u64, buffer: &'a mut [u8]) -> BlockFuture<'a, usize> {
        let result = self.inner.read_blocks(block, buffer);
        Box::pin(async move { result })
    }

    fn write_blocks<'a>(&'a self, block: u64, buffer: &'a [u8]) -> BlockFuture<'a, usize> {
        let result = self.inner.write_blocks(block, buffer);
        Box::pin(async move { result })
    }

    fn flush<'a>(&'a self) -> BlockFuture<'a, ()> {
        let result = self.inner.flush();
        Box::pin(async move { result })
    }
}

#[derive(Clone)]
pub struct BlockDeviceFile {
    device: Arc<dyn AsyncBlockDevice>,
}

impl BlockDeviceFile {
    pub fn new(device: Arc<dyn AsyncBlockDevice>) -> Self {
        Self { device }
    }

    pub fn device(&self) -> Arc<dyn AsyncBlockDevice> {
        self.device.clone()
    }
}

impl FileOperations for BlockDeviceFile {
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn read(&self, offset: usize, buffer: &mut [u8]) -> FsResult<usize> {
        let geometry = validate_geometry(self.device.geometry())?;
        let capacity = self.device.size_bytes().min(usize::MAX as u64) as usize;
        if offset >= capacity {
            return Ok(0);
        }

        let mut remaining = core::cmp::min(buffer.len(), capacity - offset);
        let mut copied = 0usize;
        let mut block = (offset / geometry.block_size) as u64;
        let mut block_offset = offset % geometry.block_size;
        let mut scratch = None::<alloc::vec::Vec<u8>>;

        while remaining > 0 {
            if block_offset == 0 {
                let direct_bytes = remaining - (remaining % geometry.block_size);
                if direct_bytes != 0 {
                    let read = block_on(
                        self.device
                            .read_blocks(block, &mut buffer[copied..copied + direct_bytes]),
                    )?;
                    if read == 0 {
                        break;
                    }

                    copied += read;
                    remaining -= read;
                    block = block.saturating_add((read / geometry.block_size) as u64);
                    if read < direct_bytes {
                        break;
                    }
                    continue;
                }
            }

            let scratch = scratch.get_or_insert_with(|| vec![0u8; geometry.block_size]);
            let available = block_on(self.device.read_blocks(block, scratch))?;
            if available <= block_offset {
                break;
            }

            let chunk = core::cmp::min(remaining, available - block_offset);
            buffer[copied..copied + chunk]
                .copy_from_slice(&scratch[block_offset..block_offset + chunk]);
            copied += chunk;
            remaining -= chunk;
            block = block.saturating_add(1);
            block_offset = 0;
        }

        Ok(copied)
    }

    fn write(&self, offset: usize, buffer: &[u8]) -> FsResult<usize> {
        let geometry = validate_geometry(self.device.geometry())?;
        let capacity = self.device.size_bytes().min(usize::MAX as u64) as usize;
        if offset >= capacity {
            return Ok(0);
        }

        let mut remaining = core::cmp::min(buffer.len(), capacity - offset);
        let mut copied = 0usize;
        let mut block = (offset / geometry.block_size) as u64;
        let mut block_offset = offset % geometry.block_size;
        let mut scratch = None::<alloc::vec::Vec<u8>>;

        while remaining > 0 {
            if block_offset == 0 {
                let direct_bytes = remaining - (remaining % geometry.block_size);
                if direct_bytes != 0 {
                    let written = block_on(
                        self.device
                            .write_blocks(block, &buffer[copied..copied + direct_bytes]),
                    )?;
                    if written == 0 {
                        break;
                    }

                    copied += written;
                    remaining -= written;
                    block = block.saturating_add((written / geometry.block_size) as u64);
                    if written < direct_bytes {
                        break;
                    }
                    continue;
                }
            }

            let scratch = scratch.get_or_insert_with(|| vec![0u8; geometry.block_size]);
            let prepared = block_on(self.device.read_blocks(block, scratch))?;
            if prepared <= block_offset {
                break;
            }

            let chunk = core::cmp::min(remaining, prepared - block_offset);
            scratch[block_offset..block_offset + chunk]
                .copy_from_slice(&buffer[copied..copied + chunk]);
            let _ = block_on(self.device.write_blocks(block, &scratch[..prepared]))?;

            copied += chunk;
            remaining -= chunk;
            block = block.saturating_add(1);
            block_offset = 0;
        }

        if copied != 0 {
            block_on(self.device.flush())?;
        }

        Ok(copied)
    }

    fn size(&self) -> usize {
        self.device.size_bytes().min(usize::MAX as u64) as usize
    }
}

fn validate_geometry(geometry: BlockGeometry) -> FsResult<BlockGeometry> {
    if geometry.is_valid() {
        Ok(geometry)
    } else {
        Err(FsError::InvalidInput)
    }
}

pub(crate) fn block_on<T>(mut future: BlockFuture<'_, T>) -> FsResult<T> {
    crate::block_on_future(async move { future.as_mut().await })
}
