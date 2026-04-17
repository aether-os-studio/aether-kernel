#![no_std]

extern crate alloc;

use alloc::sync::Arc;

use aether_device::{DeviceClass, DeviceMetadata, DeviceNode, KernelDevice, default_fbdev_name};
use aether_frame::boot::FramebufferInfo;
use aether_vfs::{
    FileNode, FileOperations, FsError, FsResult, MmapCachePolicy, MmapRequest, MmapResponse,
    NodeRef,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RgbColor {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

impl RgbColor {
    pub const BLACK: Self = Self {
        red: 0,
        green: 0,
        blue: 0,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FramebufferError {
    UnsupportedFormat,
    OutOfBounds,
}

#[derive(Debug, Clone, Copy)]
pub struct FramebufferSurface {
    base: *mut u8,
    info: FramebufferInfo,
}

unsafe impl Send for FramebufferSurface {}
unsafe impl Sync for FramebufferSurface {}

impl FramebufferSurface {
    pub fn from_boot_info(info: FramebufferInfo) -> Result<Self, FramebufferError> {
        let bytes_per_pixel = info.bits_per_pixel.div_ceil(8);
        if bytes_per_pixel == 0 || bytes_per_pixel > 4 {
            return Err(FramebufferError::UnsupportedFormat);
        }

        let base = if info.base < aether_frame::boot::hhdm_offset() {
            aether_frame::boot::phys_to_virt(info.base)
        } else {
            info.base
        };

        Ok(Self {
            base: base as *mut u8,
            info,
        })
    }

    pub fn info(&self) -> FramebufferInfo {
        self.info
    }

    pub fn physical_base(&self) -> u64 {
        if self.info.base >= aether_frame::boot::hhdm_offset() {
            self.info.base - aether_frame::boot::hhdm_offset()
        } else {
            self.info.base
        }
    }

    pub fn width(&self) -> usize {
        self.info.width as usize
    }

    pub fn height(&self) -> usize {
        self.info.height as usize
    }

    pub fn stride(&self) -> usize {
        self.info.stride as usize
    }

    pub fn bytes_per_pixel(&self) -> usize {
        self.info.bits_per_pixel.div_ceil(8) as usize
    }

    pub fn byte_len(&self) -> usize {
        self.info.size as usize
    }

    pub fn write_pixel(&self, x: usize, y: usize, color: RgbColor) -> Result<(), FramebufferError> {
        if x >= self.width() || y >= self.height() {
            return Err(FramebufferError::OutOfBounds);
        }

        let offset = y
            .checked_mul(self.stride())
            .and_then(|row| row.checked_add(x * self.bytes_per_pixel()))
            .ok_or(FramebufferError::OutOfBounds)?;
        let pixel = self.pack_color(color);

        unsafe {
            let target = self.base.add(offset);
            let bytes = pixel.to_le_bytes();
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), target, self.bytes_per_pixel());
        }
        Ok(())
    }

    pub fn write_packed_pixel(
        &self,
        x: usize,
        y: usize,
        pixel: u32,
    ) -> Result<(), FramebufferError> {
        if x >= self.width() || y >= self.height() {
            return Err(FramebufferError::OutOfBounds);
        }

        let offset = y
            .checked_mul(self.stride())
            .and_then(|row| row.checked_add(x * self.bytes_per_pixel()))
            .ok_or(FramebufferError::OutOfBounds)?;

        unsafe {
            let target = self.base.add(offset);
            let bytes = pixel.to_le_bytes();
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), target, self.bytes_per_pixel());
        }
        Ok(())
    }

    pub fn pack_color(&self, color: RgbColor) -> u32 {
        pack_color(self.info, color)
    }

    pub fn clear(&self, color: RgbColor) {
        for y in 0..self.height() {
            for x in 0..self.width() {
                let _ = self.write_pixel(x, y, color);
            }
        }
    }

    pub fn write_bytes(&self, offset: usize, buffer: &[u8]) -> FsResult<usize> {
        if offset > self.byte_len() {
            return Err(FsError::InvalidInput);
        }

        let count = core::cmp::min(buffer.len(), self.byte_len() - offset);
        unsafe {
            core::ptr::copy_nonoverlapping(buffer.as_ptr(), self.base.add(offset), count);
        }
        Ok(count)
    }

    pub fn read_bytes(&self, offset: usize, buffer: &mut [u8]) -> FsResult<usize> {
        if offset > self.byte_len() {
            return Err(FsError::InvalidInput);
        }

        let count = core::cmp::min(buffer.len(), self.byte_len() - offset);
        unsafe {
            core::ptr::copy_nonoverlapping(self.base.add(offset), buffer.as_mut_ptr(), count);
        }
        Ok(count)
    }
}

#[derive(Clone)]
pub struct FramebufferDevice {
    surface: FramebufferSurface,
    index: usize,
}

impl FramebufferDevice {
    pub fn primary(surface: FramebufferSurface) -> Self {
        Self { surface, index: 0 }
    }

    pub fn surface(&self) -> FramebufferSurface {
        self.surface
    }
}

impl KernelDevice for FramebufferDevice {
    fn metadata(&self) -> DeviceMetadata {
        DeviceMetadata::new(
            alloc::format!("fb{}", self.index),
            DeviceClass::Display,
            29,
            self.index as u16,
        )
    }

    fn nodes(&self) -> alloc::vec::Vec<DeviceNode> {
        let metadata = self.metadata();
        let node: NodeRef = FileNode::new_char_device(
            default_fbdev_name(self.index),
            u32::from(metadata.major),
            u32::from(metadata.minor),
            Arc::new(self.clone()),
        );
        alloc::vec![DeviceNode::new(default_fbdev_name(self.index), node)]
    }
}

impl FileOperations for FramebufferDevice {
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn read(&self, offset: usize, buffer: &mut [u8]) -> FsResult<usize> {
        self.surface.read_bytes(offset, buffer)
    }

    fn write(&self, offset: usize, buffer: &[u8]) -> FsResult<usize> {
        self.surface.write_bytes(offset, buffer)
    }

    fn size(&self) -> usize {
        self.surface.byte_len()
    }

    fn mmap(&self, request: MmapRequest) -> FsResult<MmapResponse> {
        let end = request
            .offset
            .checked_add(request.length)
            .ok_or(FsError::InvalidInput)?;
        if end > self.surface.byte_len() as u64 {
            return Err(FsError::InvalidInput);
        }

        Ok(MmapResponse::direct_physical(
            self.surface.physical_base().saturating_add(request.offset),
            MmapCachePolicy::Uncached,
        ))
    }
}

fn pack_color(info: FramebufferInfo, color: RgbColor) -> u32 {
    pack_component(color.red, info.pixel_layout.red)
        | pack_component(color.green, info.pixel_layout.green)
        | pack_component(color.blue, info.pixel_layout.blue)
}

fn pack_component(value: u8, bitfield: aether_frame::boot::PixelBitfield) -> u32 {
    if bitfield.size == 0 {
        return 0;
    }

    let max_value = (1u32 << bitfield.size) - 1;
    (((value as u32 * max_value) / 255) & max_value) << bitfield.shift
}
