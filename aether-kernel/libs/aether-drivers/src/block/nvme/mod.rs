mod pci;
mod registers;

extern crate alloc;

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::ToString;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::cmp::min;
use core::future::Future;
use core::pin::Pin;
use core::ptr;
use core::sync::atomic::{Ordering, compiler_fence, fence};
use core::task::{Context, Poll, Waker};

use aether_device::{DeviceClass, DeviceMetadata, DeviceNode, DeviceRegistry, KernelDevice};
use aether_frame::interrupt::Trap;
use aether_frame::libs::spin::SpinLock;
use aether_frame::mm::PAGE_SIZE;
use aether_fs::{AsyncBlockDevice, BlockFuture, BlockGeometry};
use aether_vfs::{FileNode, FsError, FsResult, NodeRef};

use crate::DmaRegion;
use crate::block::StorageDeviceHandle;

use self::pci::{
    NvmeControllerInfo, enable_bus_mastering, enable_message_interrupt, probe_controllers,
};
use self::registers::{NvmeCapabilities, NvmeRegisters};

const NVME_ADMIN_QUEUE_ID: u16 = 0;
const NVME_IO_QUEUE_ID: u16 = 1;
const NVME_QUEUE_DEPTH: u16 = 16;
const NVME_OPCODE_CREATE_IO_SQ: u8 = 0x01;
const NVME_OPCODE_CREATE_IO_CQ: u8 = 0x05;
const NVME_OPCODE_IDENTIFY: u8 = 0x06;
const NVME_OPCODE_SET_FEATURES: u8 = 0x09;
const NVME_OPCODE_WRITE: u8 = 0x01;
const NVME_OPCODE_READ: u8 = 0x02;
const NVME_FEATURE_NUMBER_OF_QUEUES: u32 = 0x07;
const NVME_IDENTIFY_CONTROLLER: u32 = 0x01;
const NVME_IDENTIFY_NAMESPACE: u32 = 0x00;
const NVME_IDENTIFY_ACTIVE_NAMESPACE_IDS: u32 = 0x02;
const NVME_CREATE_IO_CQ_PC: u32 = 1 << 0;
const NVME_CREATE_IO_CQ_IEN: u32 = 1 << 1;
const NVME_INTERRUPT_VECTOR_INDEX: u16 = 0;
const NVME_IO_COMPLETION_PHASE: u16 = 0x1;
const NVME_COMPLETION_TIMEOUT_NS: u64 = 1_000_000_000;
const NVME_CID_SLOTS: usize = 256;
const NVME_TRANSFER_PAGES: usize = 1;
const NVME_USE_POLLING_COMPLETIONS: bool = true;

static NVME_INTERRUPT_REGISTRY: SpinLock<BTreeMap<u8, Arc<NvmeTransport>>> =
    SpinLock::new(BTreeMap::new());

pub fn probe(registry: &mut DeviceRegistry) -> Vec<StorageDeviceHandle> {
    let mut devices = Vec::new();
    for (index, controller) in probe_controllers().into_iter().enumerate() {
        match NvmeController::attach(index, controller) {
            Ok(storage) => {
                registry.register(storage.clone());
                devices.push(StorageDeviceHandle {
                    name: storage.metadata().name.clone(),
                    device: storage.namespace(),
                    kernel_device: storage,
                });
            }
            Err(error) => {
                log::warn!("nvme: controller probe skipped: {:?}", error);
            }
        }
    }
    devices
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NvmeProbeError {
    RegisterAccess,
    UnsupportedVersion,
    UnsupportedPageSize,
    PciConfig,
    QueueSetup,
    QueueTimeout {
        queue_id: u16,
        cid: u16,
        cq_head: u16,
        expected_phase: bool,
    },
    QueueUnknownCid {
        queue_id: u16,
        cid: u16,
        cq_head: u16,
        phase: bool,
    },
    QueueFull {
        queue_id: u16,
        sq_head: u16,
        sq_tail: u16,
    },
    InterruptSetup,
    IdentifyController,
    IdentifyNamespaceList,
    IdentifyNamespace,
    IoQueueSetup {
        opcode: u8,
        status: u16,
    },
    AdminCommand {
        opcode: u8,
        status: u16,
    },
    IoCommand {
        opcode: u8,
        status: u16,
    },
    MissingNamespace,
    OutOfMemory,
}

struct NvmeController {
    metadata: DeviceMetadata,
    namespace: Arc<NvmeNamespace>,
}

impl NvmeController {
    fn attach(index: usize, info: NvmeControllerInfo) -> Result<Arc<Self>, NvmeProbeError> {
        enable_bus_mastering(info.address).map_err(|_| NvmeProbeError::PciConfig)?;
        let registers =
            NvmeRegisters::map(info.bar0).map_err(|_| NvmeProbeError::RegisterAccess)?;
        if registers.version().major == 0 {
            return Err(NvmeProbeError::UnsupportedVersion);
        }

        let transport = Arc::new(NvmeTransport::new(registers)?);
        if NVME_USE_POLLING_COMPLETIONS {
        } else {
            transport.enable_interrupts(info.address)?;
        }
        let namespace_info = transport
            .identify_first_namespace()
            .ok_or(NvmeProbeError::MissingNamespace)?;
        let namespace = Arc::new(NvmeNamespace::new(index, namespace_info, transport));
        let name = namespace.name();

        Ok(Arc::new(Self {
            metadata: DeviceMetadata::new(name, DeviceClass::Block, 259, index as u16),
            namespace,
        }))
    }

    fn namespace(&self) -> Arc<dyn AsyncBlockDevice> {
        self.namespace.clone()
    }
}

impl KernelDevice for NvmeController {
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

struct NvmeNamespace {
    controller_index: usize,
    info: NamespaceInfo,
    transport: Arc<NvmeTransport>,
}

impl NvmeNamespace {
    fn new(controller_index: usize, info: NamespaceInfo, transport: Arc<NvmeTransport>) -> Self {
        Self {
            controller_index,
            info,
            transport,
        }
    }

    fn name(&self) -> alloc::string::String {
        alloc::format!("nvme{}n{}", self.controller_index, self.info.namespace_id)
    }
}

impl AsyncBlockDevice for NvmeNamespace {
    fn geometry(&self) -> BlockGeometry {
        self.info.geometry
    }

    fn read_blocks<'a>(&'a self, block: u64, buffer: &'a mut [u8]) -> BlockFuture<'a, usize> {
        Box::pin(async move {
            if NVME_USE_POLLING_COMPLETIONS || !aether_frame::interrupt::are_enabled() {
                return self
                    .transport
                    .read_namespace_blocks(&self.info, block, buffer)
                    .map_err(|_| FsError::Unsupported);
            }

            NvmeReadFuture::new(self.transport.clone(), self.info, block, buffer).await
        })
    }

    fn write_blocks<'a>(&'a self, _block: u64, _buffer: &'a [u8]) -> BlockFuture<'a, usize> {
        Box::pin(async move {
            if NVME_USE_POLLING_COMPLETIONS || !aether_frame::interrupt::are_enabled() {
                return self
                    .transport
                    .write_namespace_blocks(&self.info, _block, _buffer)
                    .map_err(|_| FsError::Unsupported);
            }

            NvmeWriteFuture::new(self.transport.clone(), self.info, _block, _buffer).await
        })
    }
}

#[derive(Clone, Copy)]
struct NamespaceInfo {
    namespace_id: u32,
    geometry: BlockGeometry,
}

struct InFlightRead {
    cid: u16,
    bytes: usize,
    deadline_nanos: u64,
}

#[derive(Clone, Copy)]
struct InFlightWrite {
    cid: u16,
    bytes: usize,
    deadline_nanos: u64,
}

struct NvmeReadFuture<'a> {
    transport: Arc<NvmeTransport>,
    namespace: NamespaceInfo,
    current_lba: u64,
    buffer: &'a mut [u8],
    completed: usize,
    in_flight: Option<InFlightRead>,
}

impl<'a> NvmeReadFuture<'a> {
    fn new(
        transport: Arc<NvmeTransport>,
        namespace: NamespaceInfo,
        current_lba: u64,
        buffer: &'a mut [u8],
    ) -> Self {
        Self {
            transport,
            namespace,
            current_lba,
            buffer,
            completed: 0,
            in_flight: None,
        }
    }
}

impl Future for NvmeReadFuture<'_> {
    type Output = FsResult<usize>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let block_size = self.namespace.geometry.block_size;
        if block_size == 0 || (!self.buffer.is_empty() && self.buffer.len() % block_size != 0) {
            return Poll::Ready(Err(FsError::InvalidInput));
        }
        if self.buffer.is_empty() {
            return Poll::Ready(Ok(0));
        }

        let max_blocks_per_transfer = PAGE_SIZE as usize * NVME_TRANSFER_PAGES / block_size;
        if max_blocks_per_transfer == 0 {
            return Poll::Ready(Err(FsError::InvalidInput));
        }

        loop {
            if let Some(in_flight) = self.in_flight.as_ref() {
                let (completion, wake_list) = {
                    let mut state = self.transport.state.lock();
                    let _ = process_queue_completions_for_queue(
                        &self.transport.registers,
                        self.transport.capabilities,
                        &mut state,
                        QueueSelect::Io,
                    );
                    let timed_out = aether_frame::interrupt::timer::nanos_since_boot()
                        >= in_flight.deadline_nanos;
                    let wake_list = wake_ready_waiters(&mut state);

                    let slot = match state.requests.get_mut(in_flight.cid as usize) {
                        Some(slot) => slot,
                        None => return Poll::Ready(Err(FsError::Unsupported)),
                    };

                    if let Some(completion) = slot.completion.take() {
                        let bytes = in_flight.bytes;
                        let mut data = vec![0u8; bytes];
                        data.copy_from_slice(&slot.transfer.as_slice()[..bytes]);
                        let opcode = slot.opcode;
                        release_request(&mut state.requests, in_flight.cid);
                        (Some((completion, opcode, data)), wake_list)
                    } else if timed_out {
                        release_request(&mut state.requests, in_flight.cid);
                        return Poll::Ready(Err(FsError::Unsupported));
                    } else {
                        slot.waker = Some(cx.waker().clone());
                        (None, wake_list)
                    }
                };
                for waker in wake_list {
                    waker.wake();
                }

                let Some((completion, opcode, data)) = completion else {
                    return Poll::Pending;
                };
                fence(Ordering::Acquire);
                if ensure_io_success(opcode, completion).is_err() {
                    return Poll::Ready(Err(FsError::Unsupported));
                }

                let completed = self.completed;
                let bytes = in_flight.bytes;
                self.buffer[completed..completed + bytes].copy_from_slice(&data);
                self.completed += bytes;
                self.current_lba = self.current_lba.saturating_add((bytes / block_size) as u64);
                self.in_flight = None;
                continue;
            }

            if self.completed == self.buffer.len() {
                return Poll::Ready(Ok(self.completed));
            }

            let remaining_blocks = (self.buffer.len() - self.completed) / block_size;
            let blocks = min(remaining_blocks, max_blocks_per_transfer);
            let bytes = blocks * block_size;

            let cid = loop {
                let mut state = self.transport.state.lock();
                let cid = match alloc_cid(&mut state) {
                    Ok(cid) => cid,
                    Err(NvmeProbeError::QueueFull { .. }) => {
                        let _ = process_queue_completions_for_queue(
                            &self.transport.registers,
                            self.transport.capabilities,
                            &mut state,
                            QueueSelect::Io,
                        );
                        let wake_list = wake_ready_waiters(&mut state);
                        for waker in wake_list {
                            waker.wake();
                        }
                        if let Ok(cid) = alloc_cid(&mut state) {
                            cid
                        } else {
                            state.submit_waiters.push(cx.waker().clone());
                            return Poll::Pending;
                        }
                    }
                    Err(_) => return Poll::Ready(Err(FsError::Unsupported)),
                };

                let slot = &mut state.requests[cid as usize];
                slot.transfer.zero();
                slot.waker = Some(cx.waker().clone());
                slot.bytes = bytes;
                slot.opcode = NVME_OPCODE_READ;

                let mut command = NvmeCommand::new(NVME_OPCODE_READ, self.namespace.namespace_id);
                command.cid = cid;
                command.prp1 = slot.transfer.phys_addr();
                command.cdw10 = self.current_lba as u32;
                command.cdw11 = (self.current_lba >> 32) as u32;
                command.cdw12 = (blocks as u32).saturating_sub(1);

                if state.io.write_command(command).is_err() {
                    release_request(&mut state.requests, cid);
                    return Poll::Ready(Err(FsError::Unsupported));
                }
                fence(Ordering::Release);
                self.transport.registers.ring_submission_doorbell(
                    state.io.queue_id,
                    state.io.sq_tail,
                    self.transport.capabilities,
                );
                break cid;
            };

            self.in_flight = Some(InFlightRead {
                cid,
                bytes,
                deadline_nanos: nvme_deadline(NVME_COMPLETION_TIMEOUT_NS),
            });
            return Poll::Pending;
        }
    }
}

struct NvmeWriteFuture<'a> {
    transport: Arc<NvmeTransport>,
    namespace: NamespaceInfo,
    current_lba: u64,
    buffer: &'a [u8],
    completed: usize,
    in_flight: Option<InFlightWrite>,
}

impl<'a> NvmeWriteFuture<'a> {
    fn new(
        transport: Arc<NvmeTransport>,
        namespace: NamespaceInfo,
        current_lba: u64,
        buffer: &'a [u8],
    ) -> Self {
        Self {
            transport,
            namespace,
            current_lba,
            buffer,
            completed: 0,
            in_flight: None,
        }
    }
}

impl Future for NvmeWriteFuture<'_> {
    type Output = FsResult<usize>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let block_size = self.namespace.geometry.block_size;
        if block_size == 0 || (!self.buffer.is_empty() && self.buffer.len() % block_size != 0) {
            return Poll::Ready(Err(FsError::InvalidInput));
        }
        if self.buffer.is_empty() {
            return Poll::Ready(Ok(0));
        }

        let max_blocks_per_transfer = PAGE_SIZE as usize * NVME_TRANSFER_PAGES / block_size;
        if max_blocks_per_transfer == 0 {
            return Poll::Ready(Err(FsError::InvalidInput));
        }

        loop {
            if let Some(in_flight) = self.in_flight.as_ref() {
                let (completion, wake_list) = {
                    let mut state = self.transport.state.lock();
                    let _ = process_queue_completions_for_queue(
                        &self.transport.registers,
                        self.transport.capabilities,
                        &mut state,
                        QueueSelect::Io,
                    );
                    let timed_out = aether_frame::interrupt::timer::nanos_since_boot()
                        >= in_flight.deadline_nanos;
                    let wake_list = wake_ready_waiters(&mut state);

                    let slot = match state.requests.get_mut(in_flight.cid as usize) {
                        Some(slot) => slot,
                        None => return Poll::Ready(Err(FsError::Unsupported)),
                    };

                    if let Some(completion) = slot.completion.take() {
                        let opcode = slot.opcode;
                        release_request(&mut state.requests, in_flight.cid);
                        (Some((completion, opcode)), wake_list)
                    } else if timed_out {
                        release_request(&mut state.requests, in_flight.cid);
                        return Poll::Ready(Err(FsError::Unsupported));
                    } else {
                        slot.waker = Some(cx.waker().clone());
                        (None, wake_list)
                    }
                };
                for waker in wake_list {
                    waker.wake();
                }

                let Some((completion, opcode)) = completion else {
                    return Poll::Pending;
                };
                fence(Ordering::Acquire);
                if ensure_io_success(opcode, completion).is_err() {
                    return Poll::Ready(Err(FsError::Unsupported));
                }

                let bytes = in_flight.bytes;
                self.completed += bytes;
                self.current_lba = self.current_lba.saturating_add((bytes / block_size) as u64);
                self.in_flight = None;
                continue;
            }

            if self.completed == self.buffer.len() {
                return Poll::Ready(Ok(self.completed));
            }

            let remaining_blocks = (self.buffer.len() - self.completed) / block_size;
            let blocks = min(remaining_blocks, max_blocks_per_transfer);
            let bytes = blocks * block_size;

            let cid = loop {
                let mut state = self.transport.state.lock();
                let cid = match alloc_cid(&mut state) {
                    Ok(cid) => cid,
                    Err(NvmeProbeError::QueueFull { .. }) => {
                        let _ = process_queue_completions_for_queue(
                            &self.transport.registers,
                            self.transport.capabilities,
                            &mut state,
                            QueueSelect::Io,
                        );
                        let wake_list = wake_ready_waiters(&mut state);
                        for waker in wake_list {
                            waker.wake();
                        }
                        if let Ok(cid) = alloc_cid(&mut state) {
                            cid
                        } else {
                            state.submit_waiters.push(cx.waker().clone());
                            return Poll::Pending;
                        }
                    }
                    Err(_) => return Poll::Ready(Err(FsError::Unsupported)),
                };

                let slot = &mut state.requests[cid as usize];
                slot.transfer.zero();
                slot.transfer.as_mut_slice()[..bytes]
                    .copy_from_slice(&self.buffer[self.completed..self.completed + bytes]);
                slot.waker = Some(cx.waker().clone());
                slot.bytes = bytes;
                slot.opcode = NVME_OPCODE_WRITE;

                let mut command = NvmeCommand::new(NVME_OPCODE_WRITE, self.namespace.namespace_id);
                command.cid = cid;
                command.prp1 = slot.transfer.phys_addr();
                command.cdw10 = self.current_lba as u32;
                command.cdw11 = (self.current_lba >> 32) as u32;
                command.cdw12 = (blocks as u32).saturating_sub(1);

                if state.io.write_command(command).is_err() {
                    release_request(&mut state.requests, cid);
                    return Poll::Ready(Err(FsError::Unsupported));
                }
                fence(Ordering::Release);
                self.transport.registers.ring_submission_doorbell(
                    state.io.queue_id,
                    state.io.sq_tail,
                    self.transport.capabilities,
                );
                break cid;
            };

            self.in_flight = Some(InFlightWrite {
                cid,
                bytes,
                deadline_nanos: nvme_deadline(NVME_COMPLETION_TIMEOUT_NS),
            });
            return Poll::Pending;
        }
    }
}

#[derive(Clone, Copy)]
struct ControllerInfo {
    namespace_count: u32,
}

struct NvmeTransport {
    registers: NvmeRegisters,
    capabilities: NvmeCapabilities,
    state: SpinLock<NvmeTransportState>,
}

struct NvmeTransportState {
    admin: QueuePair,
    io: QueuePair,
    transfer: DmaRegion,
    next_cid: u16,
    requests: Vec<PendingRequest>,
    submit_waiters: Vec<Waker>,
}

struct PendingRequest {
    active: bool,
    completion: Option<NvmeCompletion>,
    transfer: DmaRegion,
    bytes: usize,
    opcode: u8,
    waker: Option<Waker>,
}

impl PendingRequest {
    fn new() -> Result<Self, NvmeProbeError> {
        Ok(Self {
            active: false,
            completion: None,
            transfer: DmaRegion::from_pages(NVME_TRANSFER_PAGES)
                .map_err(|_| NvmeProbeError::OutOfMemory)?,
            bytes: 0,
            opcode: 0,
            waker: None,
        })
    }
}

impl NvmeTransport {
    fn new(registers: NvmeRegisters) -> Result<Self, NvmeProbeError> {
        let capabilities = registers.capabilities();
        if capabilities.min_page_shift > 12 || capabilities.max_page_shift < 12 {
            return Err(NvmeProbeError::UnsupportedPageSize);
        }

        let queue_depth = min(
            NVME_QUEUE_DEPTH,
            min(
                capabilities.max_queue_entries,
                min(
                    (PAGE_SIZE as usize / 64) as u16,
                    (PAGE_SIZE as usize / 16) as u16,
                ),
            ),
        )
        .max(2);

        let mut admin = QueuePair::new(NVME_ADMIN_QUEUE_ID, queue_depth)?;
        let io = QueuePair::new(NVME_IO_QUEUE_ID, queue_depth)?;
        let transfer = DmaRegion::from_pages(1).map_err(|_| NvmeProbeError::OutOfMemory)?;

        registers.disable_controller();
        wait_ready(&registers, false, controller_ready_timeout_ns(capabilities))
            .map_err(|_| NvmeProbeError::QueueSetup)?;
        registers.set_admin_queues(queue_depth, admin.sq.phys_addr(), admin.cq.phys_addr());
        registers.enable_controller(12);
        wait_ready(&registers, true, controller_ready_timeout_ns(capabilities))
            .map_err(|_| NvmeProbeError::QueueSetup)?;
        admin.reset_phase();

        let transport = Self {
            registers,
            capabilities,
            state: SpinLock::new(NvmeTransportState {
                admin,
                io,
                transfer,
                next_cid: 1,
                requests: build_request_slots()?,
                submit_waiters: Vec::new(),
            }),
        };

        transport.create_io_queues()?;
        Ok(transport)
    }

    fn enable_interrupts(
        self: &Arc<Self>,
        address: acpi::PciAddress,
    ) -> Result<(), NvmeProbeError> {
        let vector = aether_frame::interrupt::device::allocate_vector()
            .map_err(|_| NvmeProbeError::InterruptSetup)?;
        aether_frame::interrupt::register_handler(vector, nvme_interrupt_handler)
            .map_err(|_| NvmeProbeError::InterruptSetup)?;
        enable_message_interrupt(address, vector).map_err(|_| NvmeProbeError::InterruptSetup)?;
        NVME_INTERRUPT_REGISTRY.lock().insert(vector, self.clone());
        Ok(())
    }

    fn handle_interrupt(&self) {
        let mut wake_list = Vec::new();
        {
            let mut state = self.state.lock();
            let admin_processed = process_queue_completions_for_queue(
                &self.registers,
                self.capabilities,
                &mut state,
                QueueSelect::Admin,
            )
            .unwrap_or_else(|error| {
                log::warn!(
                    "nvme: admin interrupt completion processing failed: {:?}",
                    error
                );
                0
            });
            let io_processed = process_queue_completions_for_queue(
                &self.registers,
                self.capabilities,
                &mut state,
                QueueSelect::Io,
            )
            .unwrap_or_else(|error| {
                log::warn!(
                    "nvme: io interrupt completion processing failed: {:?}",
                    error
                );
                0
            });

            if admin_processed != 0 || io_processed != 0 {
                wake_list = wake_ready_waiters(&mut state);
            }
        }

        for waker in wake_list {
            waker.wake();
        }
    }

    fn identify_first_namespace(&self) -> Option<NamespaceInfo> {
        let controller = self.identify_controller().ok()?;
        self.identify_namespaces(controller.namespace_count)
            .into_iter()
            .find_map(|namespace_id| self.identify_namespace(namespace_id).ok())
    }

    fn identify_controller(&self) -> Result<ControllerInfo, NvmeProbeError> {
        let mut state = self.state.lock();
        state.transfer.zero();

        let mut command = NvmeCommand::new(NVME_OPCODE_IDENTIFY, 0);
        command.prp1 = state.transfer.phys_addr();
        command.cdw10 = NVME_IDENTIFY_CONTROLLER;
        let completion = self.submit(&mut state, QueueSelect::Admin, command)?;
        ensure_admin_success(
            command.opcode,
            completion,
            NvmeProbeError::IdentifyController,
        )?;

        let bytes = state.transfer.as_slice();
        let namespace_count = u32::from_le_bytes(
            bytes[516..520]
                .try_into()
                .map_err(|_| NvmeProbeError::IdentifyController)?,
        );
        Ok(ControllerInfo { namespace_count })
    }

    fn identify_namespaces(&self, namespace_count: u32) -> Vec<u32> {
        let active = self
            .identify_active_namespaces()
            .ok()
            .filter(|ids| !ids.is_empty());
        match active {
            Some(ids) => ids,
            None => (1..=namespace_count).collect(),
        }
    }

    fn identify_active_namespaces(&self) -> Result<Vec<u32>, NvmeProbeError> {
        let mut state = self.state.lock();
        state.transfer.zero();

        let mut command = NvmeCommand::new(NVME_OPCODE_IDENTIFY, 0);
        command.prp1 = state.transfer.phys_addr();
        command.cdw10 = NVME_IDENTIFY_ACTIVE_NAMESPACE_IDS;
        let completion = self.submit(&mut state, QueueSelect::Admin, command)?;
        ensure_admin_success(
            command.opcode,
            completion,
            NvmeProbeError::IdentifyNamespaceList,
        )?;

        let words = state.transfer.read_words();
        let mut namespaces = Vec::new();
        for word in words {
            if word == 0 {
                break;
            }
            namespaces.push(word);
        }
        Ok(namespaces)
    }

    fn identify_namespace(&self, namespace_id: u32) -> Result<NamespaceInfo, NvmeProbeError> {
        let mut state = self.state.lock();
        state.transfer.zero();

        let mut command = NvmeCommand::new(NVME_OPCODE_IDENTIFY, namespace_id);
        command.prp1 = state.transfer.phys_addr();
        command.cdw10 = NVME_IDENTIFY_NAMESPACE;
        let completion = self.submit(&mut state, QueueSelect::Admin, command)?;
        ensure_admin_success(
            command.opcode,
            completion,
            NvmeProbeError::IdentifyNamespace,
        )?;

        let bytes = state.transfer.as_slice();
        let block_count = read_u64(bytes, 0x00);
        if block_count == 0 {
            return Err(NvmeProbeError::MissingNamespace);
        }

        let flbas = bytes[0x1a] & 0x0f;
        let lbaf = 0x80 + flbas as usize * 4;
        let block_shift = bytes
            .get(lbaf + 2)
            .copied()
            .ok_or(NvmeProbeError::IdentifyNamespace)?;
        let block_size = 1usize
            .checked_shl(block_shift as u32)
            .ok_or(NvmeProbeError::IdentifyNamespace)?;
        Ok(NamespaceInfo {
            namespace_id,
            geometry: BlockGeometry::new(block_size, block_count),
        })
    }

    fn create_io_queues(&self) -> Result<(), NvmeProbeError> {
        let mut state = self.state.lock();
        let depth = state.io.depth;

        let mut set_features = NvmeCommand::new(NVME_OPCODE_SET_FEATURES, 0);
        set_features.cdw10 = NVME_FEATURE_NUMBER_OF_QUEUES;
        set_features.cdw11 =
            (u32::from(NVME_IO_QUEUE_ID) - 1) | ((u32::from(NVME_IO_QUEUE_ID) - 1) << 16);
        let completion = self.submit(&mut state, QueueSelect::Admin, set_features)?;
        ensure_io_queue_setup_success(set_features.opcode, completion)?;

        let mut create_cq = NvmeCommand::new(NVME_OPCODE_CREATE_IO_CQ, 0);
        create_cq.prp1 = state.io.cq.phys_addr();
        create_cq.cdw10 = u32::from(NVME_IO_QUEUE_ID) | (u32::from(depth - 1) << 16);
        create_cq.cdw11 = NVME_CREATE_IO_CQ_PC;
        if !NVME_USE_POLLING_COMPLETIONS {
            create_cq.cdw11 |=
                NVME_CREATE_IO_CQ_IEN | (u32::from(NVME_INTERRUPT_VECTOR_INDEX) << 16);
        }
        let completion = self.submit(&mut state, QueueSelect::Admin, create_cq)?;
        ensure_io_queue_setup_success(create_cq.opcode, completion)?;

        let mut create_sq = NvmeCommand::new(NVME_OPCODE_CREATE_IO_SQ, 0);
        create_sq.prp1 = state.io.sq.phys_addr();
        create_sq.cdw10 = u32::from(NVME_IO_QUEUE_ID) | (u32::from(depth - 1) << 16);
        create_sq.cdw11 = (u32::from(NVME_IO_QUEUE_ID) << 16) | 0x1;
        let completion = self.submit(&mut state, QueueSelect::Admin, create_sq)?;
        ensure_io_queue_setup_success(create_sq.opcode, completion)?;

        Ok(())
    }

    fn read_namespace_blocks(
        &self,
        namespace: &NamespaceInfo,
        block: u64,
        buffer: &mut [u8],
    ) -> Result<usize, NvmeProbeError> {
        let block_size = namespace.geometry.block_size;
        if block_size == 0 || buffer.is_empty() || buffer.len() % block_size != 0 {
            return Err(NvmeProbeError::IdentifyNamespace);
        }

        let max_blocks_per_transfer = PAGE_SIZE as usize / block_size;
        if max_blocks_per_transfer == 0 {
            return Err(NvmeProbeError::UnsupportedPageSize);
        }

        let mut completed = 0usize;
        let mut current_lba = block;
        let mut remaining_blocks = buffer.len() / block_size;

        while remaining_blocks != 0 {
            let blocks = min(remaining_blocks, max_blocks_per_transfer);
            let bytes = blocks * block_size;
            let cid = {
                let mut state = self.state.lock();
                let cid = alloc_cid(&mut state)?;
                let slot = &mut state.requests[cid as usize];
                slot.transfer.zero();
                slot.bytes = bytes;
                slot.opcode = NVME_OPCODE_READ;

                let mut command = NvmeCommand::new(NVME_OPCODE_READ, namespace.namespace_id);
                command.cid = cid;
                command.prp1 = slot.transfer.phys_addr();
                command.cdw10 = current_lba as u32;
                command.cdw11 = (current_lba >> 32) as u32;
                command.cdw12 = (blocks as u32).saturating_sub(1);

                if state.io.write_command(command).is_err() {
                    release_request(&mut state.requests, cid);
                    return Err(NvmeProbeError::QueueSetup);
                }
                fence(Ordering::Release);
                self.registers.ring_submission_doorbell(
                    state.io.queue_id,
                    state.io.sq_tail,
                    self.capabilities,
                );
                cid
            };
            let completion = self.wait_for_polled_request_completion(
                QueueSelect::Io,
                cid,
                NVME_COMPLETION_TIMEOUT_NS,
            )?;
            fence(Ordering::Acquire);
            ensure_io_success(NVME_OPCODE_READ, completion)?;
            {
                let mut state = self.state.lock();
                let slot = state
                    .requests
                    .get_mut(cid as usize)
                    .ok_or(NvmeProbeError::QueueSetup)?;
                buffer[completed..completed + bytes]
                    .copy_from_slice(&slot.transfer.as_slice()[..bytes]);
                release_request(&mut state.requests, cid);
            }

            completed += bytes;
            current_lba = current_lba.saturating_add(blocks as u64);
            remaining_blocks -= blocks;
        }

        Ok(completed)
    }

    fn write_namespace_blocks(
        &self,
        namespace: &NamespaceInfo,
        block: u64,
        buffer: &[u8],
    ) -> Result<usize, NvmeProbeError> {
        let block_size = namespace.geometry.block_size;
        if block_size == 0 || buffer.is_empty() || buffer.len() % block_size != 0 {
            return Err(NvmeProbeError::IdentifyNamespace);
        }

        let max_blocks_per_transfer = PAGE_SIZE as usize / block_size;
        if max_blocks_per_transfer == 0 {
            return Err(NvmeProbeError::UnsupportedPageSize);
        }

        let mut completed = 0usize;
        let mut current_lba = block;
        let mut remaining_blocks = buffer.len() / block_size;

        while remaining_blocks != 0 {
            let blocks = min(remaining_blocks, max_blocks_per_transfer);
            let bytes = blocks * block_size;
            let cid = {
                let mut state = self.state.lock();
                let cid = alloc_cid(&mut state)?;
                let slot = &mut state.requests[cid as usize];
                slot.transfer.zero();
                slot.transfer.as_mut_slice()[..bytes]
                    .copy_from_slice(&buffer[completed..completed + bytes]);
                slot.bytes = bytes;
                slot.opcode = NVME_OPCODE_WRITE;

                let mut command = NvmeCommand::new(NVME_OPCODE_WRITE, namespace.namespace_id);
                command.cid = cid;
                command.prp1 = slot.transfer.phys_addr();
                command.cdw10 = current_lba as u32;
                command.cdw11 = (current_lba >> 32) as u32;
                command.cdw12 = (blocks as u32).saturating_sub(1);

                if state.io.write_command(command).is_err() {
                    release_request(&mut state.requests, cid);
                    return Err(NvmeProbeError::QueueSetup);
                }
                fence(Ordering::Release);
                self.registers.ring_submission_doorbell(
                    state.io.queue_id,
                    state.io.sq_tail,
                    self.capabilities,
                );
                cid
            };
            let completion = self.wait_for_polled_request_completion(
                QueueSelect::Io,
                cid,
                NVME_COMPLETION_TIMEOUT_NS,
            )?;
            fence(Ordering::Acquire);
            ensure_io_success(NVME_OPCODE_WRITE, completion)?;
            {
                let mut state = self.state.lock();
                release_request(&mut state.requests, cid);
            }

            completed += bytes;
            current_lba = current_lba.saturating_add(blocks as u64);
            remaining_blocks -= blocks;
        }

        Ok(completed)
    }

    fn wait_for_polled_request_completion(
        &self,
        queue: QueueSelect,
        cid: u16,
        timeout_ns: u64,
    ) -> Result<NvmeCompletion, NvmeProbeError> {
        let deadline = nvme_deadline(timeout_ns);
        loop {
            let poll = {
                let mut state = self.state.lock();
                process_queue_completions_for_queue(
                    &self.registers,
                    self.capabilities,
                    &mut state,
                    queue,
                )?;

                let completion = state
                    .requests
                    .get_mut(cid as usize)
                    .and_then(|slot| slot.completion.take());
                if let Some(completion) = completion {
                    return Ok(completion);
                }

                if aether_frame::interrupt::timer::nanos_since_boot() >= deadline {
                    let (queue_id, cq_head, expected_phase) = match queue {
                        QueueSelect::Admin => (
                            state.admin.queue_id,
                            state.admin.cq_head,
                            state.admin.cq_phase,
                        ),
                        QueueSelect::Io => (state.io.queue_id, state.io.cq_head, state.io.cq_phase),
                    };
                    release_request(&mut state.requests, cid);
                    return Err(NvmeProbeError::QueueTimeout {
                        queue_id,
                        cid,
                        cq_head,
                        expected_phase,
                    });
                }
                Ok::<(), NvmeProbeError>(())
            };
            poll?;
            core::hint::spin_loop();
        }
    }

    fn submit(
        &self,
        state: &mut NvmeTransportState,
        queue: QueueSelect,
        mut command: NvmeCommand,
    ) -> Result<NvmeCompletion, NvmeProbeError> {
        let cid = alloc_cid(state)?;
        command.cid = cid;

        let queue_pair = match queue {
            QueueSelect::Admin => &mut state.admin,
            QueueSelect::Io => &mut state.io,
        };
        queue_pair.write_command(command)?;
        fence(Ordering::Release);
        self.registers.ring_submission_doorbell(
            queue_pair.queue_id,
            queue_pair.sq_tail,
            self.capabilities,
        );

        let completion = wait_for_request_completion(
            &self.registers,
            self.capabilities,
            &mut state.requests,
            queue_pair,
            cid,
            NVME_COMPLETION_TIMEOUT_NS,
        )?;
        release_cid(state, cid);
        Ok(completion)
    }
}

#[derive(Clone, Copy)]
enum QueueSelect {
    Admin,
    Io,
}

struct QueuePair {
    queue_id: u16,
    depth: u16,
    sq: DmaRegion,
    cq: DmaRegion,
    sq_head: u16,
    sq_tail: u16,
    cq_head: u16,
    cq_phase: bool,
}

impl QueuePair {
    fn new(queue_id: u16, depth: u16) -> Result<Self, NvmeProbeError> {
        let mut sq = DmaRegion::from_pages(1).map_err(|_| NvmeProbeError::OutOfMemory)?;
        let mut cq = DmaRegion::from_pages(1).map_err(|_| NvmeProbeError::OutOfMemory)?;
        sq.zero();
        cq.zero();
        Ok(Self {
            queue_id,
            depth,
            sq,
            cq,
            sq_head: 0,
            sq_tail: 0,
            cq_head: 0,
            cq_phase: true,
        })
    }

    fn reset_phase(&mut self) {
        self.sq_head = 0;
        self.sq_tail = 0;
        self.cq_head = 0;
        self.cq_phase = true;
        self.sq.zero();
        self.cq.zero();
    }

    fn write_command(&mut self, command: NvmeCommand) -> Result<(), NvmeProbeError> {
        let next_tail = (self.sq_tail + 1) % self.depth;
        if next_tail == self.sq_head {
            return Err(NvmeProbeError::QueueFull {
                queue_id: self.queue_id,
                sq_head: self.sq_head,
                sq_tail: self.sq_tail,
            });
        }
        let slot = self.sq_tail as usize % self.depth as usize;
        let ptr = unsafe { self.sq.as_ptr::<NvmeCommand>().add(slot) };
        unsafe {
            ptr::write_volatile(ptr, command);
        }
        compiler_fence(Ordering::Release);
        self.sq_tail = next_tail;
        Ok(())
    }

    fn completion_ptr(&self) -> *const NvmeCompletion {
        unsafe {
            self.cq
                .as_ptr::<NvmeCompletion>()
                .add(self.cq_head as usize % self.depth as usize)
        }
    }

    fn advance_completion(&mut self) {
        self.cq_head = (self.cq_head + 1) % self.depth;
        if self.cq_head == 0 {
            self.cq_phase = !self.cq_phase;
        }
    }
}

impl DmaRegion {
    fn read_words(&self) -> alloc::vec::Vec<u32> {
        self.as_slice()
            .chunks_exact(4)
            .map(|chunk| u32::from_le_bytes(chunk.try_into().expect("u32 chunk")))
            .collect()
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct NvmeCommand {
    opcode: u8,
    flags: u8,
    cid: u16,
    nsid: u32,
    reserved0: u64,
    metadata_ptr: u64,
    prp1: u64,
    prp2: u64,
    cdw10: u32,
    cdw11: u32,
    cdw12: u32,
    cdw13: u32,
    cdw14: u32,
    cdw15: u32,
}

impl NvmeCommand {
    const fn new(opcode: u8, nsid: u32) -> Self {
        Self {
            opcode,
            flags: 0,
            cid: 0,
            nsid,
            reserved0: 0,
            metadata_ptr: 0,
            prp1: 0,
            prp2: 0,
            cdw10: 0,
            cdw11: 0,
            cdw12: 0,
            cdw13: 0,
            cdw14: 0,
            cdw15: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct NvmeCompletion {
    result: u32,
    reserved: u32,
    sq_head: u16,
    sq_id: u16,
    cid: u16,
    status: u16,
}

fn wait_ready(registers: &NvmeRegisters, expected: bool, timeout_ns: u64) -> Result<(), ()> {
    let deadline = nvme_deadline(timeout_ns);
    loop {
        if registers.controller_ready() == expected {
            return Ok(());
        }
        if aether_frame::interrupt::timer::nanos_since_boot() >= deadline {
            return Err(());
        }
        core::hint::spin_loop();
    }
}

fn wait_completion(
    _registers: &NvmeRegisters,
    _capabilities: NvmeCapabilities,
    _queue: &mut QueuePair,
    _cid: u16,
    _timeout_ns: u64,
) -> Result<NvmeCompletion, NvmeProbeError> {
    unreachable!()
}

fn wait_for_request_completion(
    registers: &NvmeRegisters,
    capabilities: NvmeCapabilities,
    requests: &mut [PendingRequest],
    queue: &mut QueuePair,
    cid: u16,
    timeout_ns: u64,
) -> Result<NvmeCompletion, NvmeProbeError> {
    let deadline = nvme_deadline(timeout_ns);
    loop {
        process_queue_completions(registers, capabilities, requests, queue)?;
        if let Some(completion) = take_request_completion(requests, cid) {
            return Ok(completion);
        }
        if aether_frame::interrupt::timer::nanos_since_boot() >= deadline {
            release_request(requests, cid);
            return Err(NvmeProbeError::QueueTimeout {
                queue_id: queue.queue_id,
                cid,
                cq_head: queue.cq_head,
                expected_phase: queue.cq_phase,
            });
        }
        core::hint::spin_loop();
    }
}

fn process_queue_completions(
    registers: &NvmeRegisters,
    capabilities: NvmeCapabilities,
    requests: &mut [PendingRequest],
    queue: &mut QueuePair,
) -> Result<usize, NvmeProbeError> {
    let mut processed = 0usize;
    loop {
        compiler_fence(Ordering::Acquire);
        let completion = unsafe { ptr::read_volatile(queue.completion_ptr()) };
        let phase = (completion.status & NVME_IO_COMPLETION_PHASE) != 0;
        if phase != queue.cq_phase {
            return Ok(processed);
        }

        let cid_index = completion.cid as usize;
        if cid_index >= requests.len() || !requests[cid_index].active {
            queue.sq_head = completion.sq_head % queue.depth;
            queue.advance_completion();
            registers.ring_completion_doorbell(queue.queue_id, queue.cq_head, capabilities);
            processed += 1;
            continue;
        }

        requests[cid_index].completion = Some(completion);
        queue.sq_head = completion.sq_head % queue.depth;
        queue.advance_completion();
        registers.ring_completion_doorbell(queue.queue_id, queue.cq_head, capabilities);
        processed += 1;
    }
}

fn wake_ready_waiters(state: &mut NvmeTransportState) -> Vec<Waker> {
    let mut wake_list = Vec::new();
    for request in &mut state.requests {
        if request.completion.is_some()
            && let Some(waker) = request.waker.take()
        {
            wake_list.push(waker);
        }
    }
    wake_list.extend(state.submit_waiters.drain(..));
    wake_list
}

fn process_queue_completions_for_queue(
    registers: &NvmeRegisters,
    capabilities: NvmeCapabilities,
    state: &mut NvmeTransportState,
    queue: QueueSelect,
) -> Result<usize, NvmeProbeError> {
    let requests = &mut state.requests as *mut Vec<PendingRequest>;
    let queue_ptr: *mut QueuePair = match queue {
        QueueSelect::Admin => &mut state.admin,
        QueueSelect::Io => &mut state.io,
    };

    unsafe { process_queue_completions(registers, capabilities, &mut *requests, &mut *queue_ptr) }
}

fn alloc_cid(state: &mut NvmeTransportState) -> Result<u16, NvmeProbeError> {
    let start = usize::from(state.next_cid.max(1));
    for step in 0..(NVME_CID_SLOTS - 1) {
        let index = 1 + ((start - 1 + step) % (NVME_CID_SLOTS - 1));
        if !state.requests[index].active {
            state.requests[index].active = true;
            state.requests[index].completion = None;
            state.requests[index].waker = None;
            state.requests[index].bytes = 0;
            state.requests[index].opcode = 0;
            state.next_cid = ((index + 1) % NVME_CID_SLOTS).max(1) as u16;
            return Ok(index as u16);
        }
    }
    Err(NvmeProbeError::QueueFull {
        queue_id: NVME_ADMIN_QUEUE_ID,
        sq_head: state.admin.sq_head,
        sq_tail: state.admin.sq_tail,
    })
}

fn take_request_completion(requests: &mut [PendingRequest], cid: u16) -> Option<NvmeCompletion> {
    let slot = requests.get_mut(cid as usize)?;
    let completion = slot.completion.take()?;
    slot.active = false;
    slot.bytes = 0;
    slot.opcode = 0;
    slot.waker = None;
    Some(completion)
}

fn release_cid(state: &mut NvmeTransportState, cid: u16) {
    release_request(&mut state.requests, cid);
}

fn release_request(requests: &mut [PendingRequest], cid: u16) {
    if let Some(slot) = requests.get_mut(cid as usize) {
        slot.active = false;
        slot.completion = None;
        slot.bytes = 0;
        slot.opcode = 0;
        slot.waker = None;
    }
}

fn build_request_slots() -> Result<Vec<PendingRequest>, NvmeProbeError> {
    let mut requests = Vec::with_capacity(NVME_CID_SLOTS);
    for _ in 0..NVME_CID_SLOTS {
        requests.push(PendingRequest::new()?);
    }
    Ok(requests)
}

fn nvme_interrupt_handler(trap: Trap, _frame: &mut aether_frame::interrupt::TrapFrame) {
    let transport = NVME_INTERRUPT_REGISTRY.lock().get(&trap.vector()).cloned();
    if let Some(transport) = transport {
        transport.handle_interrupt();
    }
}

fn completion_status_code(completion: NvmeCompletion) -> u16 {
    (completion.status >> 1) & 0x7ff
}

fn ensure_admin_success(
    opcode: u8,
    completion: NvmeCompletion,
    identify_error: NvmeProbeError,
) -> Result<(), NvmeProbeError> {
    let status = completion_status_code(completion);
    if status == 0 {
        Ok(())
    } else if opcode == NVME_OPCODE_IDENTIFY {
        Err(identify_error)
    } else {
        Err(NvmeProbeError::AdminCommand { opcode, status })
    }
}

fn ensure_io_queue_setup_success(
    opcode: u8,
    completion: NvmeCompletion,
) -> Result<(), NvmeProbeError> {
    let status = completion_status_code(completion);
    if status == 0 {
        Ok(())
    } else {
        Err(NvmeProbeError::IoQueueSetup { opcode, status })
    }
}

fn ensure_io_success(opcode: u8, completion: NvmeCompletion) -> Result<(), NvmeProbeError> {
    let status = completion_status_code(completion);
    if status == 0 {
        Ok(())
    } else {
        Err(NvmeProbeError::IoCommand { opcode, status })
    }
}

const fn controller_ready_timeout_ns(capabilities: NvmeCapabilities) -> u64 {
    let timeout_units = if capabilities.timeout_units_500ms == 0 {
        1
    } else {
        capabilities.timeout_units_500ms as u64
    };
    timeout_units * 500_000_000
}

fn nvme_deadline(timeout_ns: u64) -> u64 {
    aether_frame::interrupt::timer::nanos_since_boot().saturating_add(timeout_ns)
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    let mut raw = [0u8; 8];
    raw.copy_from_slice(&bytes[offset..offset + 8]);
    u64::from_le_bytes(raw)
}
