mod pci;

extern crate alloc;

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::ToString;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::cmp::min;
use core::ptr;

use aether_device::{DeviceClass, DeviceMetadata, DeviceNode, DeviceRegistry, KernelDevice};
use aether_frame::boot::phys_to_virt;
use aether_frame::io::{MmioRegion, remap_mmio};
use aether_frame::libs::spin::SpinLock;
use aether_frame::mm::{FrameAllocator, PAGE_SIZE, PhysFrame, frame_allocator};
use aether_fs::{AsyncBlockDevice, BlockFuture, BlockGeometry};
use aether_vfs::{FileNode, FsError, FsResult, NodeRef};
use nvme_rs::{Allocator as NvmeAllocator, Device as NvmeDevice, Error as NvmeError};

use crate::DmaRegion;
use crate::block::StorageDeviceHandle;

use self::pci::{enable_bus_mastering, probe_controllers};

const NVME_MMIO_WINDOW_BYTES: usize = 0x2000;
const NVME_IO_QUEUE_LEN: usize = 64;
static NVME_DMA_ALLOCATIONS: SpinLock<BTreeMap<usize, NvmeDmaAllocation>> =
    SpinLock::new(BTreeMap::new());

pub fn probe(registry: &mut DeviceRegistry) -> Vec<StorageDeviceHandle> {
    let mut devices = Vec::new();
    for (index, controller) in probe_controllers().into_iter().enumerate() {
        match NvmeControllerRuntime::attach(index, controller) {
            Ok(found) => {
                for device in found {
                    registry.register(device.clone());
                    devices.push(StorageDeviceHandle {
                        name: device.metadata().name.clone(),
                        device: device.namespace(),
                        kernel_device: device,
                    });
                }
            }
            Err(error) => {
                log::warn!("nvme: controller {} probe skipped: {:?}", index, error);
            }
        }
    }
    devices
}

#[derive(Debug)]
enum NvmeProbeError {
    PciConfig,
    Remap,
    Driver(NvmeError),
    MissingNamespace,
    OutOfMemory,
}

impl From<NvmeError> for NvmeProbeError {
    fn from(value: NvmeError) -> Self {
        Self::Driver(value)
    }
}

#[derive(Clone, Copy)]
struct KernelNvmeAllocator;

struct NvmeDmaAllocation {
    frame: PhysFrame,
    pages: usize,
    phys_addr: usize,
    len: usize,
}

impl NvmeAllocator for KernelNvmeAllocator {
    fn translate(&self, addr: usize) -> usize {
        let allocations = NVME_DMA_ALLOCATIONS.lock();
        if let Some((base, allocation)) = allocations.range(..=addr).next_back() {
            let offset = addr.saturating_sub(*base);
            if offset < allocation.len {
                return allocation.phys_addr + offset;
            }
        }
        let hhdm_offset = aether_frame::boot::hhdm_offset() as usize;
        assert!(
            addr >= hhdm_offset,
            "nvme allocator translate missing base for {addr:#x}"
        );
        addr - hhdm_offset
    }

    unsafe fn allocate(&self, size: usize) -> usize {
        let pages = size.div_ceil(PAGE_SIZE as usize).max(1);
        let frame = frame_allocator()
            .lock()
            .alloc(pages)
            .expect("nvme allocator out of DMA memory");
        let phys_addr = frame.start_address().as_u64() as usize;
        let virt_addr = phys_to_virt(frame.start_address().as_u64()) as usize;
        unsafe {
            ptr::write_bytes(virt_addr as *mut u8, 0, pages * PAGE_SIZE as usize);
        }
        NVME_DMA_ALLOCATIONS.lock().insert(
            virt_addr,
            NvmeDmaAllocation {
                frame,
                pages,
                phys_addr,
                len: pages * PAGE_SIZE as usize,
            },
        );
        virt_addr
    }

    unsafe fn deallocate(&self, addr: usize) {
        let Some(allocation) = NVME_DMA_ALLOCATIONS.lock().remove(&addr) else {
            panic!("nvme allocator deallocate missing allocation for {addr:#x}");
        };
        let _ = frame_allocator()
            .lock()
            .release(allocation.frame, allocation.pages);
    }
}

struct NvmeControllerRuntime {
    _mmio: MmioRegion,
    _controller: SpinLock<NvmeDevice<KernelNvmeAllocator>>,
    max_transfer_size: usize,
}

impl NvmeControllerRuntime {
    fn attach(
        controller_index: usize,
        controller: pci::NvmeControllerInfo,
    ) -> Result<Vec<Arc<NvmeBlockDevice>>, NvmeProbeError> {
        enable_bus_mastering(controller.address).map_err(|_| NvmeProbeError::PciConfig)?;
        let mmio = remap_mmio(controller.bar0, NVME_MMIO_WINDOW_BYTES)
            .map_err(|_| NvmeProbeError::Remap)?;
        let mut controller = NvmeDevice::init(mmio.base() as usize, KernelNvmeAllocator)?;
        let namespaces = controller.identify_namespaces(0)?;
        if namespaces.is_empty() {
            return Err(NvmeProbeError::MissingNamespace);
        }

        let max_transfer_size = controller
            .controller_data()
            .max_transfer_size
            .max(PAGE_SIZE as usize);
        let queue_len =
            choose_io_queue_len(controller.controller_data().max_queue_entries as usize);

        let runtime = Arc::new(Self {
            _mmio: mmio,
            _controller: SpinLock::new(controller),
            max_transfer_size,
        });

        let mut devices = Vec::with_capacity(namespaces.len());
        for namespace in namespaces {
            let block_size = usize::try_from(namespace.block_size())
                .map_err(|_| NvmeProbeError::Driver(NvmeError::InvalidBufferSize))?;
            if block_size == 0 {
                return Err(NvmeProbeError::Driver(NvmeError::InvalidBufferSize));
            }

            let transfer_bytes = aligned_transfer_bytes(runtime.max_transfer_size, block_size);
            let qpair = runtime
                ._controller
                .lock()
                .create_io_queue_pair(namespace.clone(), queue_len)?;
            let namespace_device = Arc::new(NvmeNamespaceDevice {
                controller_index,
                namespace: namespace.clone(),
                _controller: runtime.clone(),
                qpair: SpinLock::new(qpair),
                geometry: BlockGeometry::new(block_size, namespace.block_count()),
                transfer: SpinLock::new(
                    DmaRegion::new(transfer_bytes).map_err(|_| NvmeProbeError::OutOfMemory)?,
                ),
            });

            devices.push(Arc::new(NvmeBlockDevice {
                metadata: DeviceMetadata::new(
                    namespace_device.name(),
                    DeviceClass::Block,
                    259,
                    controller_index
                        .saturating_mul(32)
                        .saturating_add(devices.len()) as u16,
                ),
                namespace: namespace_device,
            }));
        }

        Ok(devices)
    }
}

struct NvmeNamespaceDevice {
    controller_index: usize,
    namespace: nvme_rs::Namespace,
    _controller: Arc<NvmeControllerRuntime>,
    qpair: SpinLock<nvme_rs::IoQueuePair<KernelNvmeAllocator>>,
    geometry: BlockGeometry,
    transfer: SpinLock<DmaRegion>,
}

impl NvmeNamespaceDevice {
    fn name(&self) -> alloc::string::String {
        alloc::format!("nvme{}n{}", self.controller_index, self.namespace.id())
    }

    fn read_blocks_sync(&self, block: u64, buffer: &mut [u8]) -> FsResult<usize> {
        let block_size = self.geometry.block_size;
        if block_size == 0 || buffer.is_empty() || buffer.len() % block_size != 0 {
            return Err(FsError::InvalidInput);
        }

        let mut qpair = self.qpair.lock();
        let transfer = self.transfer.lock();
        let max_bytes = aligned_transfer_bytes(transfer.len(), block_size);
        if max_bytes == 0 {
            return Err(FsError::InvalidInput);
        }

        let mut completed = 0usize;
        let mut current_lba = block;

        while completed < buffer.len() {
            let bytes = min(buffer.len() - completed, max_bytes);
            qpair
                .read(transfer.as_ptr::<u8>(), bytes, current_lba)
                .map_err(map_nvme_error)?;
            qpair.flush().map_err(map_nvme_error)?;
            buffer[completed..completed + bytes].copy_from_slice(&transfer.as_slice()[..bytes]);

            completed += bytes;
            current_lba = current_lba.saturating_add((bytes / block_size) as u64);
        }

        Ok(completed)
    }

    fn write_blocks_sync(&self, block: u64, buffer: &[u8]) -> FsResult<usize> {
        let block_size = self.geometry.block_size;
        if block_size == 0 || buffer.is_empty() || buffer.len() % block_size != 0 {
            return Err(FsError::InvalidInput);
        }

        let mut qpair = self.qpair.lock();
        let mut transfer = self.transfer.lock();
        let max_bytes = aligned_transfer_bytes(transfer.len(), block_size);
        if max_bytes == 0 {
            return Err(FsError::InvalidInput);
        }

        let mut completed = 0usize;
        let mut current_lba = block;

        while completed < buffer.len() {
            let bytes = min(buffer.len() - completed, max_bytes);
            transfer.as_mut_slice()[..bytes].copy_from_slice(&buffer[completed..completed + bytes]);
            qpair
                .write(transfer.as_ptr::<u8>(), bytes, current_lba)
                .map_err(map_nvme_error)?;
            qpair.flush().map_err(map_nvme_error)?;

            completed += bytes;
            current_lba = current_lba.saturating_add((bytes / block_size) as u64);
        }

        Ok(completed)
    }
}

impl AsyncBlockDevice for NvmeNamespaceDevice {
    fn geometry(&self) -> BlockGeometry {
        self.geometry
    }

    fn read_blocks<'a>(&'a self, block: u64, buffer: &'a mut [u8]) -> BlockFuture<'a, usize> {
        Box::pin(async move { self.read_blocks_sync(block, buffer) })
    }

    fn write_blocks<'a>(&'a self, block: u64, buffer: &'a [u8]) -> BlockFuture<'a, usize> {
        Box::pin(async move { self.write_blocks_sync(block, buffer) })
    }
}

struct NvmeBlockDevice {
    metadata: DeviceMetadata,
    namespace: Arc<NvmeNamespaceDevice>,
}

impl NvmeBlockDevice {
    fn namespace(&self) -> Arc<dyn AsyncBlockDevice> {
        self.namespace.clone()
    }
}

impl KernelDevice for NvmeBlockDevice {
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn metadata(&self) -> DeviceMetadata {
        self.metadata.clone()
    }

    fn nodes(&self) -> Vec<DeviceNode> {
        let metadata = self.metadata();
        let file: NodeRef = FileNode::new_block_device(
            self.namespace.name().to_string(),
            u32::from(metadata.major),
            u32::from(metadata.minor),
            Arc::new(aether_fs::BlockDeviceFile::new(self.namespace())),
        );
        vec![DeviceNode::new(self.namespace.name(), file)]
    }
}

fn choose_io_queue_len(max_queue_entries: usize) -> usize {
    max_queue_entries.clamp(2, NVME_IO_QUEUE_LEN)
}

fn aligned_transfer_bytes(limit: usize, block_size: usize) -> usize {
    let limit = limit.max(block_size);
    let aligned = limit - (limit % block_size);
    aligned.max(block_size)
}

fn map_nvme_error(error: NvmeError) -> FsError {
    match error {
        NvmeError::SubQueueFull => FsError::WouldBlock,
        NvmeError::InvalidBufferSize
        | NvmeError::NotAlignedToDword
        | NvmeError::NotAlignedToPage
        | NvmeError::IoSizeExceedsMdts
        | NvmeError::QueueSizeTooSmall
        | NvmeError::QueueSizeExceedsMqes
        | NvmeError::CommandFailed(_) => FsError::Unsupported,
    }
}
