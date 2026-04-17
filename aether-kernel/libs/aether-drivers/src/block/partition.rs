extern crate alloc;

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::future::Future;
use core::pin::Pin;

use aether_device::{
    AsyncBlockDevice, DeviceClass, DeviceMetadata, DeviceNode, DeviceRegistry, KernelDevice,
};
use aether_fs::{BlockDeviceFile, BlockFuture, BlockGeometry};
use aether_vfs::{FileNode, FsError, NodeRef};

use crate::block::StorageDeviceHandle;

const MBR_SIGNATURE_OFFSET: usize = 510;
const MBR_PARTITION_TABLE_OFFSET: usize = 446;
const MBR_PARTITION_ENTRY_SIZE: usize = 16;
const GPT_HEADER_LBA: u64 = 1;
const GPT_SIGNATURE: &[u8; 8] = b"EFI PART";
const GPT_ENTRY_SIZE_MIN: u32 = 128;
const GPT_ENTRY_SIZE_MAX: u32 = 4096;
const GPT_ENTRY_COUNT_MAX: u32 = 1024;
const EXTENDED_PARTITION_TYPES: [u8; 3] = [0x05, 0x0f, 0x85];

pub fn probe(
    registry: &mut DeviceRegistry,
    devices: &[StorageDeviceHandle],
) -> Vec<StorageDeviceHandle> {
    let mut partitions = Vec::new();
    for device in devices {
        let parent_metadata = device.kernel_device.metadata();
        let entries = match parse_partitions(device.device.clone()) {
            Ok(entries) => entries,
            Err(_) => continue,
        };

        for entry in entries {
            let name = alloc::format!("{}part{}", device.name, entry.index);
            let minor = partition_minor(parent_metadata.minor, entry.index);
            let partition_device = Arc::new(PartitionDevice::new(
                DeviceMetadata::new(
                    name.clone(),
                    DeviceClass::Block,
                    parent_metadata.major,
                    minor,
                ),
                Arc::new(PartitionBlockDevice::new(device.device.clone(), entry)),
            ));
            registry.register(partition_device.clone());
            partitions.push(StorageDeviceHandle {
                name,
                device: partition_device.device(),
                kernel_device: partition_device,
            });
        }
    }
    partitions
}

#[derive(Clone, Copy)]
struct PartitionEntry {
    index: u32,
    first_lba: u64,
    block_count: u64,
}

struct PartitionBlockDevice {
    parent: Arc<dyn AsyncBlockDevice>,
    entry: PartitionEntry,
}

impl PartitionBlockDevice {
    const fn new(parent: Arc<dyn AsyncBlockDevice>, entry: PartitionEntry) -> Self {
        Self { parent, entry }
    }
}

impl AsyncBlockDevice for PartitionBlockDevice {
    fn geometry(&self) -> BlockGeometry {
        BlockGeometry::new(self.parent.geometry().block_size, self.entry.block_count)
    }

    fn read_blocks<'a>(&'a self, block: u64, buffer: &'a mut [u8]) -> BlockFuture<'a, usize> {
        self.parent
            .read_blocks(self.entry.first_lba.saturating_add(block), buffer)
    }

    fn write_blocks<'a>(&'a self, block: u64, buffer: &'a [u8]) -> BlockFuture<'a, usize> {
        self.parent
            .write_blocks(self.entry.first_lba.saturating_add(block), buffer)
    }

    fn flush<'a>(&'a self) -> BlockFuture<'a, ()> {
        self.parent.flush()
    }
}

struct PartitionDevice {
    metadata: DeviceMetadata,
    device: Arc<PartitionBlockDevice>,
}

impl PartitionDevice {
    fn new(metadata: DeviceMetadata, device: Arc<PartitionBlockDevice>) -> Self {
        Self { metadata, device }
    }

    fn device(&self) -> Arc<dyn AsyncBlockDevice> {
        self.device.clone()
    }
}

impl KernelDevice for PartitionDevice {
    fn metadata(&self) -> DeviceMetadata {
        self.metadata.clone()
    }

    fn nodes(&self) -> Vec<DeviceNode> {
        let metadata = self.metadata();
        let file: NodeRef = FileNode::new_block_device(
            metadata.name.clone(),
            u32::from(metadata.major),
            u32::from(metadata.minor),
            Arc::new(BlockDeviceFile::new(self.device())),
        );
        Vec::from([DeviceNode::new(metadata.name, file)])
    }
}

fn parse_partitions(device: Arc<dyn AsyncBlockDevice>) -> Result<Vec<PartitionEntry>, ()> {
    let geometry = device.geometry();
    if geometry.block_size < 512 || !geometry.is_valid() {
        return Err(());
    }

    if let Ok(entries) = parse_gpt(device.clone(), geometry) {
        if !entries.is_empty() {
            return Ok(entries);
        }
    }

    match parse_mbr(device.clone(), geometry) {
        Ok(entries) if !entries.is_empty() => Ok(entries),
        _ => Ok(Vec::from([whole_device_partition(geometry)])),
    }
}

fn whole_device_partition(geometry: BlockGeometry) -> PartitionEntry {
    PartitionEntry {
        index: 1,
        first_lba: 0,
        block_count: geometry.block_count,
    }
}

fn parse_gpt(
    device: Arc<dyn AsyncBlockDevice>,
    geometry: BlockGeometry,
) -> Result<Vec<PartitionEntry>, ()> {
    let header = read_lba(device.clone(), GPT_HEADER_LBA, geometry.block_size)?;
    if header.get(..8) != Some(GPT_SIGNATURE) {
        return Err(());
    }

    let entry_lba = read_u64(&header, 72)?;
    let entry_count = read_u32(&header, 80)?;
    let entry_size = read_u32(&header, 84)?;
    if !(GPT_ENTRY_SIZE_MIN..=GPT_ENTRY_SIZE_MAX).contains(&entry_size) {
        return Err(());
    }
    if entry_count == 0 || entry_count > GPT_ENTRY_COUNT_MAX {
        return Err(());
    }

    let table_bytes = entry_count as usize * entry_size as usize;
    let entry_blocks = table_bytes.div_ceil(geometry.block_size);
    let table = read_lba_span(device, entry_lba, entry_blocks, geometry.block_size)?;

    let mut partitions = Vec::new();
    for index in 0..entry_count as usize {
        let offset = index * entry_size as usize;
        let entry = table.get(offset..offset + entry_size as usize).ok_or(())?;
        if entry[..16].iter().all(|byte| *byte == 0) {
            continue;
        }

        let first_lba = read_u64(entry, 32)?;
        let last_lba = read_u64(entry, 40)?;
        if first_lba == 0 || last_lba < first_lba {
            continue;
        }

        partitions.push(PartitionEntry {
            index: index as u32 + 1,
            first_lba,
            block_count: last_lba.saturating_sub(first_lba).saturating_add(1),
        });
    }

    Ok(partitions)
}

fn parse_mbr(
    device: Arc<dyn AsyncBlockDevice>,
    geometry: BlockGeometry,
) -> Result<Vec<PartitionEntry>, ()> {
    let sector0 = read_lba(device.clone(), 0, geometry.block_size)?;
    if sector0.get(MBR_SIGNATURE_OFFSET..MBR_SIGNATURE_OFFSET + 2) != Some(&[0x55, 0xaa]) {
        return Err(());
    }

    let mut partitions = Vec::new();
    let mut next_index = 1u32;

    for slot in 0..4usize {
        let offset = MBR_PARTITION_TABLE_OFFSET + slot * MBR_PARTITION_ENTRY_SIZE;
        let entry = parse_mbr_entry(&sector0[offset..offset + MBR_PARTITION_ENTRY_SIZE])?;
        if entry.block_count == 0 {
            continue;
        }

        if EXTENDED_PARTITION_TYPES.contains(&entry.partition_type) {
            parse_extended_partitions(
                device.clone(),
                geometry,
                entry.first_lba,
                &mut next_index,
                &mut partitions,
            )?;
            continue;
        }

        partitions.push(PartitionEntry {
            index: next_index,
            first_lba: entry.first_lba,
            block_count: entry.block_count,
        });
        next_index = next_index.saturating_add(1);
    }

    Ok(partitions)
}

fn parse_extended_partitions(
    device: Arc<dyn AsyncBlockDevice>,
    geometry: BlockGeometry,
    extended_base_lba: u64,
    next_index: &mut u32,
    partitions: &mut Vec<PartitionEntry>,
) -> Result<(), ()> {
    let mut ebr_lba = extended_base_lba;
    loop {
        let sector = read_lba(device.clone(), ebr_lba, geometry.block_size)?;
        if sector.get(MBR_SIGNATURE_OFFSET..MBR_SIGNATURE_OFFSET + 2) != Some(&[0x55, 0xaa]) {
            return Ok(());
        }

        let logical = parse_mbr_entry(
            &sector
                [MBR_PARTITION_TABLE_OFFSET..MBR_PARTITION_TABLE_OFFSET + MBR_PARTITION_ENTRY_SIZE],
        )?;
        if logical.block_count != 0 {
            partitions.push(PartitionEntry {
                index: *next_index,
                first_lba: ebr_lba.saturating_add(logical.first_lba),
                block_count: logical.block_count,
            });
            *next_index = next_index.saturating_add(1);
        }

        let next = parse_mbr_entry(
            &sector[MBR_PARTITION_TABLE_OFFSET + MBR_PARTITION_ENTRY_SIZE
                ..MBR_PARTITION_TABLE_OFFSET + 2 * MBR_PARTITION_ENTRY_SIZE],
        )?;
        if next.block_count == 0 || !EXTENDED_PARTITION_TYPES.contains(&next.partition_type) {
            return Ok(());
        }
        ebr_lba = extended_base_lba.saturating_add(next.first_lba);
    }
}

#[derive(Clone, Copy)]
struct MbrPartitionEntry {
    partition_type: u8,
    first_lba: u64,
    block_count: u64,
}

fn parse_mbr_entry(bytes: &[u8]) -> Result<MbrPartitionEntry, ()> {
    Ok(MbrPartitionEntry {
        partition_type: *bytes.get(4).ok_or(())?,
        first_lba: read_u32(bytes, 8)? as u64,
        block_count: read_u32(bytes, 12)? as u64,
    })
}

fn read_lba(device: Arc<dyn AsyncBlockDevice>, lba: u64, block_size: usize) -> Result<Vec<u8>, ()> {
    let mut buffer = vec![0u8; block_size];
    let read = block_on(device.read_blocks(lba, &mut buffer)).map_err(|_| ())?;
    if read < block_size {
        return Err(());
    }
    Ok(buffer)
}

fn read_lba_span(
    device: Arc<dyn AsyncBlockDevice>,
    lba: u64,
    blocks: usize,
    block_size: usize,
) -> Result<Vec<u8>, ()> {
    let mut buffer = vec![0u8; blocks.saturating_mul(block_size)];
    let read = block_on(device.read_blocks(lba, &mut buffer)).map_err(|_| ())?;
    if read < buffer.len() {
        return Err(());
    }
    Ok(buffer)
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, ()> {
    let raw = bytes.get(offset..offset + 4).ok_or(())?;
    Ok(u32::from_le_bytes(raw.try_into().map_err(|_| ())?))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, ()> {
    let raw = bytes.get(offset..offset + 8).ok_or(())?;
    Ok(u64::from_le_bytes(raw.try_into().map_err(|_| ())?))
}

fn partition_minor(parent_minor: u16, index: u32) -> u16 {
    let value = (u32::from(parent_minor) << 4).saturating_add(index);
    value.min(u16::MAX as u32) as u16
}

fn block_on<T>(
    mut future: Pin<Box<dyn Future<Output = Result<T, FsError>> + Send + '_>>,
) -> Result<T, FsError> {
    aether_frame::executor::block_on(async move { future.as_mut().await })
}
