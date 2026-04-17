extern crate alloc;

use aether_framebuffer::FramebufferSurface;

use super::core::{
    DRM_FORMAT_ABGR8888, DRM_FORMAT_ARGB8888, DRM_FORMAT_XBGR8888, DRM_FORMAT_XRGB8888,
    DrmDriverInfo, DrmIoctlError, DrmModeInfo, DrmScanoutBackend, color_from_pixel,
};

pub struct PlainFbBackend {
    surface: FramebufferSurface,
}

impl PlainFbBackend {
    pub fn new(surface: FramebufferSurface) -> Self {
        Self { surface }
    }

    fn is_native_format(&self, pixel_format: u32) -> bool {
        let info = self.surface.info();
        info.bits_per_pixel == 32
            && matches!(
                pixel_format,
                DRM_FORMAT_XRGB8888
                    | DRM_FORMAT_ARGB8888
                    | DRM_FORMAT_XBGR8888
                    | DRM_FORMAT_ABGR8888
            )
            && ((pixel_format == DRM_FORMAT_XRGB8888 || pixel_format == DRM_FORMAT_ARGB8888)
                && info.pixel_layout.red.shift == 16
                && info.pixel_layout.red.size == 8
                && info.pixel_layout.green.shift == 8
                && info.pixel_layout.green.size == 8
                && info.pixel_layout.blue.shift == 0
                && info.pixel_layout.blue.size == 8
                || (pixel_format == DRM_FORMAT_XBGR8888 || pixel_format == DRM_FORMAT_ABGR8888)
                    && info.pixel_layout.red.shift == 0
                    && info.pixel_layout.red.size == 8
                    && info.pixel_layout.green.shift == 8
                    && info.pixel_layout.green.size == 8
                    && info.pixel_layout.blue.shift == 16
                    && info.pixel_layout.blue.size == 8)
    }
}

impl DrmScanoutBackend for PlainFbBackend {
    fn driver_info(&self) -> DrmDriverInfo {
        DrmDriverInfo {
            name: alloc::string::String::from("simpledrm"),
            date: alloc::string::String::from("20260417"),
            description: alloc::string::String::from("Aether plain framebuffer DRM"),
        }
    }

    fn mode(&self) -> DrmModeInfo {
        DrmModeInfo::simple(
            self.surface.width() as u32,
            self.surface.height() as u32,
            60,
        )
    }

    fn mm_size(&self) -> (u32, u32) {
        let width = ((self.surface.width() as u32).saturating_mul(264) / 1000).max(1);
        let height = ((self.surface.height() as u32).saturating_mul(264) / 1000).max(1);
        (width, height)
    }

    fn present(
        &self,
        bytes: &[u8],
        width: u32,
        height: u32,
        pitch: u32,
        pixel_format: u32,
    ) -> Result<(), DrmIoctlError> {
        let width = width.min(self.surface.width() as u32) as usize;
        let height = height.min(self.surface.height() as u32) as usize;
        let pitch = pitch as usize;
        if pitch == 0 {
            return Err(DrmIoctlError::Invalid);
        }

        if self.is_native_format(pixel_format) {
            let row_bytes = width.saturating_mul(4);
            if pitch == self.surface.stride() && height.saturating_mul(row_bytes) <= bytes.len() {
                self.surface
                    .write_bytes(0, &bytes[..height.saturating_mul(row_bytes)])
                    .map_err(|_| DrmIoctlError::Invalid)?;
                return Ok(());
            }
            for row in 0..height {
                let start = row.saturating_mul(pitch);
                let end = start.saturating_add(row_bytes);
                if end > bytes.len() {
                    return Err(DrmIoctlError::Invalid);
                }
                let dst_offset = row.saturating_mul(self.surface.stride());
                self.surface
                    .write_bytes(dst_offset, &bytes[start..end])
                    .map_err(|_| DrmIoctlError::Invalid)?;
            }
            return Ok(());
        }

        for y in 0..height {
            let row = y.saturating_mul(pitch);
            for x in 0..width {
                let start = row.saturating_add(x.saturating_mul(4));
                let end = start.saturating_add(4);
                if end > bytes.len() {
                    return Err(DrmIoctlError::Invalid);
                }
                let pixel = u32::from_le_bytes(
                    bytes[start..end]
                        .try_into()
                        .map_err(|_| DrmIoctlError::Invalid)?,
                );
                let color =
                    color_from_pixel(pixel_format, pixel).ok_or(DrmIoctlError::NotSupported)?;
                self.surface
                    .write_pixel(x, y, color)
                    .map_err(|_| DrmIoctlError::Invalid)?;
            }
        }
        Ok(())
    }
}
