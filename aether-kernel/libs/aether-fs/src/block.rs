extern crate alloc;

use alloc::boxed::Box;
use alloc::sync::Arc;
use core::future::Future;
use core::hint::spin_loop;
use core::pin::Pin;
use core::ptr;

use aether_frame::boot::phys_to_virt;
use aether_frame::libs::spin::SpinLock;
use aether_frame::mm::{FrameAllocator, PAGE_SIZE, PhysFrame, frame_allocator};
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

    fn max_transfer_bytes(&self) -> usize {
        self.geometry().block_size
    }

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

    fn acquire_transfer_buffer(&self, min_len: usize) -> TransferBufferLease {
        TransferBufferLease::owned(min_len.max(self.geometry().block_size))
    }
}

pub trait SyncBlockDevice: Send + Sync {
    fn geometry(&self) -> BlockGeometry;

    fn max_transfer_bytes(&self) -> usize {
        self.geometry().block_size
    }

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

    fn max_transfer_bytes(&self) -> usize {
        self.inner.max_transfer_bytes()
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

pub struct TransferBufferLease {
    buffer: Option<TransferBufferRegion>,
    pool: Option<Arc<TransferBufferPool>>,
}

impl TransferBufferLease {
    fn owned(len: usize) -> Self {
        Self::new(
            TransferBufferRegion::new(len).expect("transfer buffer allocation must succeed"),
            None,
        )
    }

    fn new(buffer: TransferBufferRegion, pool: Option<Arc<TransferBufferPool>>) -> Self {
        Self {
            buffer: Some(buffer),
            pool,
        }
    }

    pub fn as_mut_slice(&mut self, len: usize) -> &mut [u8] {
        self.buffer
            .as_mut()
            .expect("transfer buffer lease must own a buffer")
            .as_mut_slice(len)
    }
}

impl Drop for TransferBufferLease {
    fn drop(&mut self) {
        let Some(buffer) = self.buffer.take() else {
            return;
        };
        let Some(pool) = self.pool.take() else {
            return;
        };
        pool.release(buffer);
    }
}

struct TransferBufferRegion {
    frame: PhysFrame,
    pages: usize,
    len: usize,
    ptr: *mut u8,
}

unsafe impl Send for TransferBufferRegion {}
unsafe impl Sync for TransferBufferRegion {}

impl TransferBufferRegion {
    fn new(len: usize) -> Result<Self, FsError> {
        let pages = len.div_ceil(PAGE_SIZE as usize).max(1);
        let frame = frame_allocator()
            .lock()
            .alloc(pages)
            .map_err(|_| FsError::Unsupported)?;
        let ptr = phys_to_virt(frame.start_address().as_u64()) as *mut u8;
        unsafe {
            ptr::write_bytes(ptr, 0, pages * PAGE_SIZE as usize);
        }
        Ok(Self {
            frame,
            pages,
            len: pages * PAGE_SIZE as usize,
            ptr,
        })
    }

    const fn len(&self) -> usize {
        self.len
    }

    fn as_mut_slice(&mut self, len: usize) -> &mut [u8] {
        assert!(
            len <= self.len,
            "transfer buffer slice exceeds region length"
        );
        unsafe { core::slice::from_raw_parts_mut(self.ptr, len) }
    }
}

impl Drop for TransferBufferRegion {
    fn drop(&mut self) {
        let _ = frame_allocator().lock().release(self.frame, self.pages);
    }
}

struct TransferBufferPool {
    slot: SpinLock<Option<TransferBufferRegion>>,
    base_len: usize,
}

impl TransferBufferPool {
    fn new(base_len: usize) -> Result<Self, FsError> {
        Ok(Self {
            slot: SpinLock::new(Some(TransferBufferRegion::new(base_len)?)),
            base_len,
        })
    }

    fn acquire(pool: &Arc<Self>, min_len: usize) -> TransferBufferLease {
        let desired = pool.base_len.max(min_len);
        assert!(
            desired <= pool.base_len,
            "transfer buffer request exceeds preallocated region"
        );

        loop {
            if let Some(buffer) = pool.slot.lock().take() {
                return TransferBufferLease::new(buffer, Some(pool.clone()));
            }
            spin_loop();
        }
    }

    fn release(&self, buffer: TransferBufferRegion) {
        debug_assert!(buffer.len() >= self.base_len);
        let mut slot = self.slot.lock();
        *slot = Some(buffer);
    }
}

struct BufferedBlockDevice {
    inner: Arc<dyn AsyncBlockDevice>,
    transfer: Arc<TransferBufferPool>,
    geometry: BlockGeometry,
    max_transfer_bytes: usize,
}

impl BufferedBlockDevice {
    fn new(inner: Arc<dyn AsyncBlockDevice>) -> Self {
        let geometry = inner.geometry();
        let max_transfer_bytes = inner.max_transfer_bytes().max(geometry.block_size);
        Self {
            inner,
            transfer: Arc::new(
                TransferBufferPool::new(max_transfer_bytes)
                    .expect("block transfer buffer allocation must succeed"),
            ),
            geometry,
            max_transfer_bytes,
        }
    }
}

impl AsyncBlockDevice for BufferedBlockDevice {
    fn geometry(&self) -> BlockGeometry {
        self.geometry
    }

    fn max_transfer_bytes(&self) -> usize {
        self.max_transfer_bytes
    }

    fn acquire_transfer_buffer(&self, min_len: usize) -> TransferBufferLease {
        TransferBufferPool::acquire(&self.transfer, min_len)
    }

    fn read_blocks<'a>(&'a self, block: u64, buffer: &'a mut [u8]) -> BlockFuture<'a, usize> {
        self.inner.read_blocks(block, buffer)
    }

    fn write_blocks<'a>(&'a self, block: u64, buffer: &'a [u8]) -> BlockFuture<'a, usize> {
        self.inner.write_blocks(block, buffer)
    }

    fn flush<'a>(&'a self) -> BlockFuture<'a, ()> {
        self.inner.flush()
    }
}

#[derive(Clone)]
pub struct BlockDeviceFile {
    device: Arc<dyn AsyncBlockDevice>,
}

impl BlockDeviceFile {
    pub fn new(device: Arc<dyn AsyncBlockDevice>) -> Self {
        Self {
            device: Arc::new(BufferedBlockDevice::new(device)),
        }
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
        let mut scratch = None::<TransferBufferLease>;

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

            let scratch = scratch
                .get_or_insert_with(|| self.device.acquire_transfer_buffer(geometry.block_size));
            let available = block_on(
                self.device
                    .read_blocks(block, scratch.as_mut_slice(geometry.block_size)),
            )?;
            if available <= block_offset {
                break;
            }

            let chunk = core::cmp::min(remaining, available - block_offset);
            let scratch = scratch.as_mut_slice(available);
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
        let mut scratch = None::<TransferBufferLease>;

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

            let scratch = scratch
                .get_or_insert_with(|| self.device.acquire_transfer_buffer(geometry.block_size));
            let prepared = block_on(
                self.device
                    .read_blocks(block, scratch.as_mut_slice(geometry.block_size)),
            )?;
            if prepared <= block_offset {
                break;
            }

            let chunk = core::cmp::min(remaining, prepared - block_offset);
            let scratch = scratch.as_mut_slice(prepared);
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
