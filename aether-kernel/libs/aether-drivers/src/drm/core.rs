extern crate alloc;

use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::{String, ToString};
use alloc::sync::{Arc, Weak};
use alloc::vec;
use alloc::vec::Vec;
use core::any::Any;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use aether_device::{DeviceClass, DeviceMetadata, DeviceNode, DeviceRegistry, KernelDevice};
use aether_frame::libs::spin::SpinLock;
use aether_frame::time;
use aether_framebuffer::{FramebufferSurface, RgbColor};
use aether_vfs::{
    FileNode, FileOperations, FsError, FsResult, MmapCachePolicy, MmapRequest, MmapResponse,
    NodeRef, PollEvents, SharedWaitListener, WaitQueue,
};

use super::plainfb::PlainFbBackend;
use crate::DmaRegion;

pub const DRM_MAJOR: u16 = 226;
pub const DRM_MODE_CONNECTED: u32 = 1;
pub const DRM_MODE_ENCODER_VIRTUAL: u32 = 5;
pub const DRM_MODE_CONNECTOR_VIRTUAL: u32 = 15;
pub const DRM_MODE_OBJECT_CRTC: u32 = 0xcccc_cccc;
pub const DRM_MODE_OBJECT_CONNECTOR: u32 = 0xc0c0_c0c0;
pub const DRM_MODE_OBJECT_ENCODER: u32 = 0xe0e0_e0e0;
pub const DRM_MODE_OBJECT_FB: u32 = 0xfbfb_fbfb;
pub const DRM_MODE_OBJECT_PLANE: u32 = 0xeeee_eeee;
pub const DRM_MODE_OBJECT_ANY: u32 = 0;

pub const DRM_MODE_PROP_RANGE: u32 = 1 << 1;
pub const DRM_MODE_PROP_IMMUTABLE: u32 = 1 << 2;
pub const DRM_MODE_PROP_ENUM: u32 = 1 << 3;
pub const DRM_MODE_PROP_BLOB: u32 = 1 << 4;
pub const DRM_MODE_PROP_OBJECT: u32 = 1 << 6;
pub const DRM_MODE_PROP_SIGNED_RANGE: u32 = 2 << 6;
pub const DRM_MODE_PROP_ATOMIC: u32 = 0x8000_0000;

pub const DRM_MODE_DPMS_ON: u64 = 0;
pub const DRM_MODE_DPMS_STANDBY: u64 = 1;
pub const DRM_MODE_DPMS_SUSPEND: u64 = 2;
pub const DRM_MODE_DPMS_OFF: u64 = 3;

pub const DRM_PLANE_TYPE_PRIMARY: u64 = 1;
pub const DRM_PLANE_TYPE_OVERLAY: u64 = 0;
pub const DRM_PLANE_TYPE_CURSOR: u64 = 2;

pub const DRM_PROPERTY_ID_FB_ID: u32 = 3;
pub const DRM_PROPERTY_ID_CRTC_X: u32 = 5;
pub const DRM_PROPERTY_ID_CRTC_Y: u32 = 6;
pub const DRM_PROPERTY_ID_PLANE_TYPE: u32 = 7;
pub const DRM_PROPERTY_ID_CRTC_ID: u32 = 9;
pub const DRM_PROPERTY_ID_SRC_W: u32 = 10;
pub const DRM_PROPERTY_ID_SRC_X: u32 = 11;
pub const DRM_PROPERTY_ID_SRC_Y: u32 = 12;
pub const DRM_PROPERTY_ID_CRTC_W: u32 = 13;
pub const DRM_PROPERTY_ID_CRTC_H: u32 = 14;
pub const DRM_PROPERTY_ID_IN_FORMATS: u32 = 15;
pub const DRM_PROPERTY_ID_SRC_H: u32 = 16;
pub const DRM_CRTC_ACTIVE_PROP_ID: u32 = 0x100;
pub const DRM_CRTC_MODE_ID_PROP_ID: u32 = 0x101;
pub const DRM_CONNECTOR_DPMS_PROP_ID: u32 = 0x200;
pub const DRM_CONNECTOR_EDID_PROP_ID: u32 = 0x201;
pub const DRM_CONNECTOR_CRTC_ID_PROP_ID: u32 = 0x202;
pub const DRM_FB_WIDTH_PROP_ID: u32 = 0x300;
pub const DRM_FB_HEIGHT_PROP_ID: u32 = 0x301;
pub const DRM_FB_BPP_PROP_ID: u32 = 0x302;
pub const DRM_FB_DEPTH_PROP_ID: u32 = 0x303;

pub const DRM_CAP_DUMB_BUFFER: u64 = 0x1;
pub const DRM_CAP_DUMB_PREFERRED_DEPTH: u64 = 0x3;
pub const DRM_CAP_DUMB_PREFER_SHADOW: u64 = 0x4;
pub const DRM_CAP_TIMESTAMP_MONOTONIC: u64 = 0x6;
pub const DRM_CAP_CURSOR_WIDTH: u64 = 0x8;
pub const DRM_CAP_CURSOR_HEIGHT: u64 = 0x9;
pub const DRM_CAP_ADDFB2_MODIFIERS: u64 = 0x10;
pub const DRM_CAP_PAGE_FLIP_TARGET: u64 = 0x11;
pub const DRM_CAP_CRTC_IN_VBLANK_EVENT: u64 = 0x12;

pub const DRM_CLIENT_CAP_STEREO_3D: u64 = 1;
pub const DRM_CLIENT_CAP_UNIVERSAL_PLANES: u64 = 2;
pub const DRM_CLIENT_CAP_ATOMIC: u64 = 3;
pub const DRM_CLIENT_CAP_ASPECT_RATIO: u64 = 4;
pub const DRM_CLIENT_CAP_WRITEBACK_CONNECTORS: u64 = 5;

pub const DRM_EVENT_FLIP_COMPLETE: u32 = 0x02;
pub const DRM_EVENT_VBLANK: u32 = 0x01;
pub const DRM_MODE_FB_DIRTY_ANNOTATE_COPY: u32 = 0x01;
pub const DRM_MODE_FB_DIRTY_ANNOTATE_FILL: u32 = 0x02;
pub const DRM_MODE_FB_DIRTY_FLAGS: u32 =
    DRM_MODE_FB_DIRTY_ANNOTATE_COPY | DRM_MODE_FB_DIRTY_ANNOTATE_FILL;

pub const DRM_MODE_PAGE_FLIP_EVENT: u32 = 0x01;
pub const DRM_MODE_PAGE_FLIP_FLAGS: u32 = DRM_MODE_PAGE_FLIP_EVENT | 0x02 | 0x0c;

pub const DRM_FORMAT_XRGB8888: u32 = fourcc_code(b'X', b'R', b'2', b'4');
pub const DRM_FORMAT_XBGR8888: u32 = fourcc_code(b'X', b'B', b'2', b'4');
pub const DRM_FORMAT_ARGB8888: u32 = fourcc_code(b'A', b'R', b'2', b'4');
pub const DRM_FORMAT_ABGR8888: u32 = fourcc_code(b'A', b'B', b'2', b'4');

const DEFAULT_DRM_FORMATS: [u32; 4] = [
    DRM_FORMAT_XRGB8888,
    DRM_FORMAT_ARGB8888,
    DRM_FORMAT_XBGR8888,
    DRM_FORMAT_ABGR8888,
];
const DRM_EVENT_BYTES: usize = 32;
const MAX_READY_DRM_EVENTS: usize = 64;
const MAX_PENDING_DRM_EVENTS: usize = 256;

static DRM_DEVICES: SpinLock<Vec<Weak<DrmDevice>>> = SpinLock::new(Vec::new());
static NEXT_VBLANK_DEADLINE_NS: AtomicU64 = AtomicU64::new(u64::MAX);
static USER_PROPERTY_BLOBS: SpinLock<BTreeMap<u32, Vec<u8>>> = SpinLock::new(BTreeMap::new());
static NEXT_USER_PROPERTY_BLOB_ID: AtomicU32 = AtomicU32::new(0x3000_0000);

const DRM_BLOB_MODE_BASE: u32 = 0x1000_0000;
const DRM_BLOB_EDID_BASE: u32 = 0x1100_0000;
const DRM_BLOB_IN_FORMATS_BASE: u32 = 0x1200_0000;
const FORMAT_BLOB_CURRENT: u32 = 1;

pub const fn fourcc_code(a: u8, b: u8, c: u8, d: u8) -> u32 {
    (a as u32) | ((b as u32) << 8) | ((c as u32) << 16) | ((d as u32) << 24)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DrmIoctlError {
    Invalid,
    NotFound,
    Busy,
    Permission,
    NotSupported,
    NoMemory,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrmDriverInfo {
    pub name: String,
    pub date: String,
    pub description: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrmModeInfo {
    pub clock: u32,
    pub hdisplay: u16,
    pub hsync_start: u16,
    pub hsync_end: u16,
    pub htotal: u16,
    pub hskew: u16,
    pub vdisplay: u16,
    pub vsync_start: u16,
    pub vsync_end: u16,
    pub vtotal: u16,
    pub vscan: u16,
    pub vrefresh: u32,
    pub flags: u32,
    pub mode_type: u32,
    pub name: [u8; 32],
}

impl DrmModeInfo {
    pub fn simple(width: u32, height: u32, refresh_hz: u32) -> Self {
        let mut name = [0u8; 32];
        let rendered = alloc::format!("{width}x{height}");
        let bytes = rendered.as_bytes();
        let len = bytes.len().min(name.len().saturating_sub(1));
        name[..len].copy_from_slice(&bytes[..len]);
        let hsync_start = width.saturating_add(16);
        let hsync_end = width.saturating_add(16 + 96);
        let htotal = width.saturating_add(16 + 96 + 48);
        let vsync_start = height.saturating_add(10);
        let vsync_end = height.saturating_add(12);
        let vtotal = height.saturating_add(45);
        let clock = htotal
            .saturating_mul(vtotal)
            .saturating_mul(refresh_hz)
            .div_ceil(1000);
        Self {
            clock,
            hdisplay: width as u16,
            hsync_start: hsync_start as u16,
            hsync_end: hsync_end as u16,
            htotal: htotal as u16,
            hskew: 0,
            vdisplay: height as u16,
            vsync_start: vsync_start as u16,
            vsync_end: vsync_end as u16,
            vtotal: vtotal as u16,
            vscan: 0,
            vrefresh: refresh_hz,
            flags: 0,
            mode_type: 0,
            name,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrmResourcesSnapshot {
    pub framebuffer_ids: Vec<u32>,
    pub crtc_ids: Vec<u32>,
    pub connector_ids: Vec<u32>,
    pub encoder_ids: Vec<u32>,
    pub min_width: u32,
    pub max_width: u32,
    pub min_height: u32,
    pub max_height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrmCrtcSnapshot {
    pub crtc_id: u32,
    pub framebuffer_id: u32,
    pub x: u32,
    pub y: u32,
    pub gamma_size: u32,
    pub mode_valid: bool,
    pub mode: DrmModeInfo,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrmEncoderSnapshot {
    pub encoder_id: u32,
    pub encoder_type: u32,
    pub crtc_id: u32,
    pub possible_crtcs: u32,
    pub possible_clones: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrmConnectorSnapshot {
    pub connector_id: u32,
    pub encoder_id: u32,
    pub connector_type: u32,
    pub connector_type_id: u32,
    pub connection: u32,
    pub mm_width: u32,
    pub mm_height: u32,
    pub subpixel: u32,
    pub modes: Vec<DrmModeInfo>,
    pub encoders: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrmPlaneSnapshot {
    pub plane_id: u32,
    pub crtc_id: u32,
    pub framebuffer_id: u32,
    pub possible_crtcs: u32,
    pub gamma_size: u32,
    pub format_types: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrmFramebufferSnapshot {
    pub framebuffer_id: u32,
    pub width: u32,
    pub height: u32,
    pub pitch: u32,
    pub depth: u32,
    pub bpp: u32,
    pub handle: u32,
    pub pixel_format: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrmFramebufferCreate {
    pub width: u32,
    pub height: u32,
    pub pixel_format: u32,
    pub flags: u32,
    pub handle: u32,
    pub pitch: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrmObjectPropertiesSnapshot {
    pub ids: Vec<u32>,
    pub values: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrmPropertyEnumValue {
    pub value: u64,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrmPropertyInfo {
    pub prop_id: u32,
    pub flags: u32,
    pub name: String,
    pub values: Vec<u64>,
    pub enums: Vec<DrmPropertyEnumValue>,
}

pub trait DrmScanoutBackend: Send + Sync {
    fn driver_info(&self) -> DrmDriverInfo;
    fn mode(&self) -> DrmModeInfo;
    fn mm_size(&self) -> (u32, u32);
    fn supported_formats(&self) -> &'static [u32] {
        &DEFAULT_DRM_FORMATS
    }
    fn present(
        &self,
        bytes: &[u8],
        width: u32,
        height: u32,
        pitch: u32,
        pixel_format: u32,
    ) -> Result<(), DrmIoctlError>;
}

struct DumbBuffer {
    handle: u32,
    width: u32,
    height: u32,
    bpp: u32,
    pitch: u32,
    size: usize,
    dma: DmaRegion,
}

impl DumbBuffer {
    fn phys_addr(&self) -> u64 {
        self.dma.phys_addr()
    }

    fn bytes(&self) -> &[u8] {
        &self.dma.as_slice()[..self.size]
    }
}

struct DrmFramebufferState {
    id: u32,
    width: u32,
    height: u32,
    pitch: u32,
    depth: u32,
    bpp: u32,
    handle: u32,
    pixel_format: u32,
}

#[derive(Debug, Clone, Copy)]
struct PendingDrmEvent {
    type_: u32,
    user_data: u64,
    target_sequence: u64,
}

struct DrmState {
    next_handle: u32,
    next_fb_id: u32,
    current_fb_id: u32,
    plane_crtc_id: u32,
    connector_crtc_id: u32,
    crtc_x: u32,
    crtc_y: u32,
    crtc_w: u32,
    crtc_h: u32,
    crtc_mode_valid: bool,
    crtc_mode: DrmModeInfo,
    master_pid: Option<u32>,
    universal_planes: bool,
    dumb_buffers: BTreeMap<u32, Arc<DumbBuffer>>,
    framebuffers: BTreeMap<u32, DrmFramebufferState>,
    event_queue: VecDeque<[u8; DRM_EVENT_BYTES]>,
    pending_events: VecDeque<PendingDrmEvent>,
    vblank_sequence: u64,
    vblank_period_ns: u64,
    next_vblank_ns: u64,
}

pub struct DrmDevice {
    backend: Arc<dyn DrmScanoutBackend>,
    index: usize,
    driver_info: DrmDriverInfo,
    connector_id: u32,
    crtc_id: u32,
    encoder_id: u32,
    plane_id: u32,
    waiters: WaitQueue,
    version: AtomicU64,
    state: SpinLock<DrmState>,
}

impl DrmDevice {
    fn crtc_mode_blob_id(&self) -> u32 {
        DRM_BLOB_MODE_BASE | self.crtc_id
    }

    fn connector_edid_blob_id(&self) -> u32 {
        DRM_BLOB_EDID_BASE | self.connector_id
    }

    fn plane_in_formats_blob_id(&self) -> u32 {
        DRM_BLOB_IN_FORMATS_BASE | self.plane_id
    }

    fn build_connector_edid(&self) -> Vec<u8> {
        let mode = self.backend.mode();
        let (mm_width, mm_height) = self.backend.mm_size();
        let width_cm = (mm_width / 10).clamp(1, u32::from(u8::MAX)) as u8;
        let height_cm = (mm_height / 10).clamp(1, u32::from(u8::MAX)) as u8;
        let width = mode.hdisplay as u32;
        let height = mode.vdisplay as u32;
        let refresh = mode.vrefresh.max(1);

        let mut edid = [0u8; 128];
        edid[0..8].copy_from_slice(&[0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00]);
        edid[8..10].copy_from_slice(&0x4d54u16.to_be_bytes());
        edid[10..12].copy_from_slice(&0x0001u16.to_le_bytes());
        edid[12..16].copy_from_slice(&1u32.to_le_bytes());
        edid[16] = 1;
        edid[17] = 4;
        edid[18] = 1;
        edid[19] = 4;
        edid[20] = 0x80;
        edid[21] = width_cm;
        edid[22] = height_cm;
        edid[23] = 0x78;
        edid[24..35].fill(0);
        edid[35] = 0x01;
        edid[36] = 0x01;
        edid[37] = 0x01;
        edid[38] = 0x01;
        edid[39] = 0x01;
        edid[40] = 0x01;
        edid[41] = 0x01;
        edid[42] = 0x01;
        edid[43] = 0x01;
        edid[44] = 0x01;
        edid[45] = 0x01;
        edid[46] = 0x01;
        edid[47] = 0x01;
        edid[48] = 0x01;
        edid[49] = 0x01;
        edid[50] = 0x01;

        let hsync_start = width.saturating_add(16);
        let hsync_end = width.saturating_add(16 + 96);
        let htotal = width.saturating_add(16 + 96 + 48);
        let vsync_start = height.saturating_add(10);
        let vsync_end = height.saturating_add(12);
        let vtotal = height.saturating_add(45);
        let hblank = htotal.saturating_sub(width);
        let vblank = vtotal.saturating_sub(height);
        let hsync_offset = hsync_start.saturating_sub(width);
        let hsync_pulse = hsync_end.saturating_sub(hsync_start);
        let vsync_offset = vsync_start.saturating_sub(height);
        let vsync_pulse = vsync_end.saturating_sub(vsync_start);
        let pixel_clock = htotal
            .saturating_mul(vtotal)
            .saturating_mul(refresh)
            .div_ceil(10_000);
        let dtd = &mut edid[54..72];
        dtd[0..2].copy_from_slice(&(pixel_clock as u16).to_le_bytes());
        dtd[2] = (width & 0xff) as u8;
        dtd[3] = (hblank & 0xff) as u8;
        dtd[4] = (((width >> 8) & 0x0f) << 4 | ((hblank >> 8) & 0x0f)) as u8;
        dtd[5] = (height & 0xff) as u8;
        dtd[6] = (vblank & 0xff) as u8;
        dtd[7] = (((height >> 8) & 0x0f) << 4 | ((vblank >> 8) & 0x0f)) as u8;
        dtd[8] = (hsync_offset & 0xff) as u8;
        dtd[9] = (hsync_pulse & 0xff) as u8;
        dtd[10] = (((vsync_offset & 0x0f) << 4) | (vsync_pulse & 0x0f)) as u8;
        dtd[11] = ((((hsync_offset >> 8) & 0x03) << 6)
            | (((hsync_pulse >> 8) & 0x03) << 4)
            | (((vsync_offset >> 4) & 0x03) << 2)
            | ((vsync_pulse >> 4) & 0x03)) as u8;
        dtd[12] = width_cm.saturating_mul(10);
        dtd[13] = height_cm.saturating_mul(10);
        dtd[14] = 0;
        dtd[15] = 0;
        dtd[16] = 0;
        dtd[17] = 0x1a;

        edid[72..90].copy_from_slice(&[
            0x00, 0x00, 0x00, 0xfc, 0x00, b'A', b'e', b't', b'h', b'e', b'r', b'-', b'F', b'B',
            0x0a, 0x20, 0x20, 0x20,
        ]);
        edid[90..108].copy_from_slice(&[
            0x00, 0x00, 0x00, 0xff, 0x00, b'A', b'E', b'T', b'H', b'E', b'R', b'0', b'0', b'0',
            b'1', 0x0a, 0x20, 0x20,
        ]);
        edid[108..126].copy_from_slice(&[
            0x00, 0x00, 0x00, 0xfd, 0x00, 0x1e, 0x78, 0x1e, 0xff, 0x00, 0x0a, 0x20, 0x20, 0x20,
            0x20, 0x20, 0x20, 0x20,
        ]);
        let checksum = edid[..127]
            .iter()
            .fold(0u8, |sum, byte| sum.wrapping_add(*byte));
        edid[127] = checksum.wrapping_neg();
        edid.to_vec()
    }

    fn build_in_formats_blob(&self) -> Vec<u8> {
        let formats = self.backend.supported_formats();
        let formats_len = core::mem::size_of_val(formats);
        let modifiers_offset = 24 + formats_len;
        let mut bytes = vec![0u8; modifiers_offset + 24];
        bytes[0..4].copy_from_slice(&FORMAT_BLOB_CURRENT.to_ne_bytes());
        bytes[8..12].copy_from_slice(&(formats.len() as u32).to_ne_bytes());
        bytes[12..16].copy_from_slice(&24u32.to_ne_bytes());
        bytes[16..20].copy_from_slice(&1u32.to_ne_bytes());
        bytes[20..24].copy_from_slice(&(modifiers_offset as u32).to_ne_bytes());
        for (index, format) in formats.iter().copied().enumerate() {
            let offset = 24 + index * 4;
            bytes[offset..offset + 4].copy_from_slice(&format.to_ne_bytes());
        }
        let mask = if formats.len() >= 64 {
            u64::MAX
        } else {
            (1u64 << formats.len()) - 1
        };
        bytes[modifiers_offset..modifiers_offset + 8].copy_from_slice(&mask.to_ne_bytes());
        bytes[modifiers_offset + 8..modifiers_offset + 12].copy_from_slice(&0u32.to_ne_bytes());
        bytes[modifiers_offset + 12..modifiers_offset + 16].copy_from_slice(&0u32.to_ne_bytes());
        bytes[modifiers_offset + 16..modifiers_offset + 24].copy_from_slice(&0u64.to_ne_bytes());
        bytes
    }

    pub fn get_property_blob(&self, blob_id: u32) -> Option<Vec<u8>> {
        if blob_id == self.crtc_mode_blob_id() {
            let mut bytes = [0u8; DrmModeInfo::SIZE];
            self.state
                .lock()
                .crtc_mode
                .write_to_bytes(&mut bytes)
                .then_some(bytes.to_vec())
        } else if blob_id == self.connector_edid_blob_id() {
            Some(self.build_connector_edid())
        } else if blob_id == self.plane_in_formats_blob_id() {
            Some(self.build_in_formats_blob())
        } else {
            USER_PROPERTY_BLOBS.lock().get(&blob_id).cloned()
        }
    }

    pub fn create_property_blob(&self, bytes: &[u8]) -> Result<u32, DrmIoctlError> {
        let blob_id = NEXT_USER_PROPERTY_BLOB_ID.fetch_add(1, Ordering::AcqRel);
        USER_PROPERTY_BLOBS.lock().insert(blob_id, bytes.to_vec());
        Ok(blob_id)
    }

    pub fn destroy_property_blob(&self, blob_id: u32) -> Result<(), DrmIoctlError> {
        if blob_id == self.crtc_mode_blob_id()
            || blob_id == self.connector_edid_blob_id()
            || blob_id == self.plane_in_formats_blob_id()
        {
            return Err(DrmIoctlError::Permission);
        }
        USER_PROPERTY_BLOBS
            .lock()
            .remove(&blob_id)
            .map(|_| ())
            .ok_or(DrmIoctlError::NotFound)
    }

    fn mode_from_blob(&self, blob_id: u32) -> Result<DrmModeInfo, DrmIoctlError> {
        if blob_id == self.crtc_mode_blob_id() {
            return Ok(self.state.lock().crtc_mode);
        }
        let bytes = self
            .get_property_blob(blob_id)
            .ok_or(DrmIoctlError::NotFound)?;
        DrmModeInfo::from_bytes(&bytes).ok_or(DrmIoctlError::Invalid)
    }

    pub fn new(index: usize, backend: Arc<dyn DrmScanoutBackend>) -> Arc<Self> {
        let mode = backend.mode();
        let refresh_hz = mode.vrefresh.max(1) as u64;
        let vblank_period_ns = 1_000_000_000u64 / refresh_hz;
        let next_vblank_ns = time::monotonic_nanos().saturating_add(vblank_period_ns);
        let device = Arc::new(Self {
            driver_info: backend.driver_info(),
            backend,
            index,
            connector_id: 1,
            crtc_id: 2,
            encoder_id: 3,
            plane_id: 4,
            waiters: WaitQueue::new(),
            version: AtomicU64::new(1),
            state: SpinLock::new(DrmState {
                next_handle: 1,
                next_fb_id: 16,
                current_fb_id: 0,
                plane_crtc_id: 2,
                connector_crtc_id: 2,
                crtc_x: 0,
                crtc_y: 0,
                crtc_w: u32::from(mode.hdisplay),
                crtc_h: u32::from(mode.vdisplay),
                crtc_mode_valid: true,
                crtc_mode: mode,
                master_pid: None,
                universal_planes: false,
                dumb_buffers: BTreeMap::new(),
                framebuffers: BTreeMap::new(),
                event_queue: VecDeque::new(),
                pending_events: VecDeque::new(),
                vblank_sequence: 0,
                vblank_period_ns,
                next_vblank_ns,
            }),
        });
        DRM_DEVICES.lock().push(Arc::downgrade(&device));
        refresh_next_vblank_deadline();
        device
    }

    fn bump(&self) {
        let _ = self.version.fetch_add(1, Ordering::AcqRel);
    }

    pub fn driver_info(&self) -> &DrmDriverInfo {
        &self.driver_info
    }

    pub fn version(&self) -> u64 {
        self.version.load(Ordering::Acquire)
    }

    pub fn index(&self) -> usize {
        self.index
    }

    pub fn get_cap(&self, capability: u64) -> u64 {
        match capability {
            DRM_CAP_DUMB_BUFFER => 1,
            DRM_CAP_DUMB_PREFERRED_DEPTH => 24,
            DRM_CAP_DUMB_PREFER_SHADOW => 1,
            DRM_CAP_TIMESTAMP_MONOTONIC => 1,
            DRM_CAP_CRTC_IN_VBLANK_EVENT => 1,
            DRM_CAP_ADDFB2_MODIFIERS => 0,
            DRM_CAP_PAGE_FLIP_TARGET => 0,
            DRM_CAP_CURSOR_WIDTH | DRM_CAP_CURSOR_HEIGHT => 0,
            _ => 0,
        }
    }

    pub fn set_client_cap(&self, capability: u64, value: u64) -> Result<(), DrmIoctlError> {
        match capability {
            DRM_CLIENT_CAP_STEREO_3D
            | DRM_CLIENT_CAP_ASPECT_RATIO
            | DRM_CLIENT_CAP_WRITEBACK_CONNECTORS => {
                if value == 0 || value == 1 {
                    return Ok(());
                }
                Err(DrmIoctlError::Invalid)
            }
            DRM_CLIENT_CAP_UNIVERSAL_PLANES => {
                if value > 1 {
                    return Err(DrmIoctlError::Invalid);
                }
                self.state.lock().universal_planes = value != 0;
                Ok(())
            }
            DRM_CLIENT_CAP_ATOMIC => {
                if value <= 1 {
                    return Ok(());
                }
                Err(DrmIoctlError::Invalid)
            }
            _ => Err(DrmIoctlError::Invalid),
        }
    }

    pub fn set_master(&self, pid: u32) -> Result<(), DrmIoctlError> {
        let mut state = self.state.lock();
        match state.master_pid {
            Some(owner) if owner != pid => Err(DrmIoctlError::Busy),
            _ => {
                state.master_pid = Some(pid);
                Ok(())
            }
        }
    }

    pub fn drop_master(&self, pid: u32) -> Result<(), DrmIoctlError> {
        let mut state = self.state.lock();
        match state.master_pid {
            Some(owner) if owner == pid => {
                state.master_pid = None;
                Ok(())
            }
            Some(_) => Err(DrmIoctlError::Permission),
            None => Ok(()),
        }
    }

    pub fn resources(&self) -> DrmResourcesSnapshot {
        let state = self.state.lock();
        let mode = self.backend.mode();
        DrmResourcesSnapshot {
            framebuffer_ids: state.framebuffers.keys().copied().collect(),
            crtc_ids: alloc::vec![self.crtc_id],
            connector_ids: alloc::vec![self.connector_id],
            encoder_ids: alloc::vec![self.encoder_id],
            min_width: u32::from(mode.hdisplay),
            max_width: u32::from(mode.hdisplay),
            min_height: u32::from(mode.vdisplay),
            max_height: u32::from(mode.vdisplay),
        }
    }

    pub fn get_crtc(&self, crtc_id: u32) -> Option<DrmCrtcSnapshot> {
        (crtc_id == self.crtc_id).then(|| {
            let state = self.state.lock();
            DrmCrtcSnapshot {
                crtc_id: self.crtc_id,
                framebuffer_id: state.current_fb_id,
                x: state.crtc_x,
                y: state.crtc_y,
                gamma_size: 0,
                mode_valid: state.crtc_mode_valid,
                mode: state.crtc_mode,
            }
        })
    }

    pub fn get_encoder(&self, encoder_id: u32) -> Option<DrmEncoderSnapshot> {
        (encoder_id == self.encoder_id).then_some(DrmEncoderSnapshot {
            encoder_id: self.encoder_id,
            encoder_type: DRM_MODE_ENCODER_VIRTUAL,
            crtc_id: self.crtc_id,
            possible_crtcs: 1,
            possible_clones: 0,
        })
    }

    pub fn get_connector(&self, connector_id: u32) -> Option<DrmConnectorSnapshot> {
        (connector_id == self.connector_id).then(|| {
            let mode = self.backend.mode();
            let (mm_width, mm_height) = self.backend.mm_size();
            DrmConnectorSnapshot {
                connector_id: self.connector_id,
                encoder_id: self.encoder_id,
                connector_type: DRM_MODE_CONNECTOR_VIRTUAL,
                connector_type_id: 1,
                connection: DRM_MODE_CONNECTED,
                mm_width,
                mm_height,
                subpixel: 0,
                modes: alloc::vec![mode],
                encoders: alloc::vec![self.encoder_id],
            }
        })
    }

    pub fn plane_ids(&self) -> Vec<u32> {
        let _ = self.state.lock().universal_planes;
        alloc::vec![self.plane_id]
    }

    pub fn get_plane(&self, plane_id: u32) -> Option<DrmPlaneSnapshot> {
        (plane_id == self.plane_id).then(|| {
            let state = self.state.lock();
            DrmPlaneSnapshot {
                plane_id: self.plane_id,
                crtc_id: state.plane_crtc_id,
                framebuffer_id: state.current_fb_id,
                possible_crtcs: 1,
                gamma_size: 0,
                format_types: self.backend.supported_formats().to_vec(),
            }
        })
    }

    pub fn get_framebuffer(&self, framebuffer_id: u32) -> Option<DrmFramebufferSnapshot> {
        let state = self.state.lock();
        state
            .framebuffers
            .get(&framebuffer_id)
            .map(|fb| DrmFramebufferSnapshot {
                framebuffer_id: fb.id,
                width: fb.width,
                height: fb.height,
                pitch: fb.pitch,
                depth: fb.depth,
                bpp: fb.bpp,
                handle: fb.handle,
                pixel_format: fb.pixel_format,
            })
    }

    pub fn get_object_properties(
        &self,
        object_id: u32,
        object_type: u32,
    ) -> Result<DrmObjectPropertiesSnapshot, DrmIoctlError> {
        let resolved_type = if object_type == DRM_MODE_OBJECT_ANY {
            if object_id == self.crtc_id {
                DRM_MODE_OBJECT_CRTC
            } else if object_id == self.connector_id {
                DRM_MODE_OBJECT_CONNECTOR
            } else if object_id == self.encoder_id {
                DRM_MODE_OBJECT_ENCODER
            } else if object_id == self.plane_id {
                DRM_MODE_OBJECT_PLANE
            } else if self.get_framebuffer(object_id).is_some() {
                DRM_MODE_OBJECT_FB
            } else {
                return Err(DrmIoctlError::NotFound);
            }
        } else {
            object_type
        };

        match resolved_type {
            DRM_MODE_OBJECT_CRTC => {
                if object_id != self.crtc_id {
                    return Err(DrmIoctlError::NotFound);
                }
                let state = self.state.lock();
                Ok(DrmObjectPropertiesSnapshot {
                    ids: alloc::vec![DRM_CRTC_ACTIVE_PROP_ID, DRM_CRTC_MODE_ID_PROP_ID],
                    values: alloc::vec![
                        u64::from(state.crtc_mode_valid),
                        u64::from(self.crtc_mode_blob_id()),
                    ],
                })
            }
            DRM_MODE_OBJECT_CONNECTOR => {
                if object_id != self.connector_id {
                    return Err(DrmIoctlError::NotFound);
                }
                let state = self.state.lock();
                Ok(DrmObjectPropertiesSnapshot {
                    ids: alloc::vec![
                        DRM_CONNECTOR_DPMS_PROP_ID,
                        DRM_CONNECTOR_EDID_PROP_ID,
                        DRM_CONNECTOR_CRTC_ID_PROP_ID,
                    ],
                    values: alloc::vec![
                        DRM_MODE_DPMS_ON,
                        u64::from(self.connector_edid_blob_id()),
                        u64::from(state.connector_crtc_id),
                    ],
                })
            }
            DRM_MODE_OBJECT_PLANE => {
                let snapshot = self.get_plane(object_id).ok_or(DrmIoctlError::NotFound)?;
                let (crtc_x, crtc_y, crtc_w, crtc_h) = {
                    let state = self.state.lock();
                    (state.crtc_x, state.crtc_y, state.crtc_w, state.crtc_h)
                };
                let (src_w, src_h) = self
                    .get_framebuffer(snapshot.framebuffer_id)
                    .map(|fb| ((u64::from(fb.width)) << 16, (u64::from(fb.height)) << 16))
                    .unwrap_or_else(|| {
                        if snapshot.crtc_id != 0 {
                            ((u64::from(crtc_w)) << 16, (u64::from(crtc_h)) << 16)
                        } else {
                            (0, 0)
                        }
                    });
                Ok(DrmObjectPropertiesSnapshot {
                    ids: alloc::vec![
                        DRM_PROPERTY_ID_PLANE_TYPE,
                        DRM_PROPERTY_ID_IN_FORMATS,
                        DRM_PROPERTY_ID_FB_ID,
                        DRM_PROPERTY_ID_CRTC_ID,
                        DRM_PROPERTY_ID_SRC_X,
                        DRM_PROPERTY_ID_SRC_Y,
                        DRM_PROPERTY_ID_SRC_W,
                        DRM_PROPERTY_ID_SRC_H,
                        DRM_PROPERTY_ID_CRTC_X,
                        DRM_PROPERTY_ID_CRTC_Y,
                        DRM_PROPERTY_ID_CRTC_W,
                        DRM_PROPERTY_ID_CRTC_H,
                    ],
                    values: alloc::vec![
                        DRM_PLANE_TYPE_PRIMARY,
                        u64::from(self.plane_in_formats_blob_id()),
                        u64::from(snapshot.framebuffer_id),
                        u64::from(snapshot.crtc_id),
                        0,
                        0,
                        src_w,
                        src_h,
                        if snapshot.crtc_id != 0 {
                            u64::from(crtc_x)
                        } else {
                            0
                        },
                        if snapshot.crtc_id != 0 {
                            u64::from(crtc_y)
                        } else {
                            0
                        },
                        if snapshot.crtc_id != 0 {
                            u64::from(crtc_w)
                        } else {
                            0
                        },
                        if snapshot.crtc_id != 0 {
                            u64::from(crtc_h)
                        } else {
                            0
                        },
                    ],
                })
            }
            DRM_MODE_OBJECT_ENCODER => {
                if object_id != self.encoder_id {
                    return Err(DrmIoctlError::NotFound);
                }
                Ok(DrmObjectPropertiesSnapshot {
                    ids: Vec::new(),
                    values: Vec::new(),
                })
            }
            DRM_MODE_OBJECT_FB => {
                let snapshot = self
                    .get_framebuffer(object_id)
                    .ok_or(DrmIoctlError::NotFound)?;
                Ok(DrmObjectPropertiesSnapshot {
                    ids: alloc::vec![
                        DRM_FB_WIDTH_PROP_ID,
                        DRM_FB_HEIGHT_PROP_ID,
                        DRM_FB_BPP_PROP_ID,
                        DRM_FB_DEPTH_PROP_ID,
                    ],
                    values: alloc::vec![
                        u64::from(snapshot.width),
                        u64::from(snapshot.height),
                        u64::from(snapshot.bpp),
                        u64::from(snapshot.depth),
                    ],
                })
            }
            _ => Err(DrmIoctlError::NotSupported),
        }
    }

    pub fn get_property(&self, prop_id: u32) -> Result<DrmPropertyInfo, DrmIoctlError> {
        match prop_id {
            DRM_PROPERTY_ID_FB_ID => Ok(DrmPropertyInfo {
                prop_id,
                flags: DRM_MODE_PROP_OBJECT | DRM_MODE_PROP_ATOMIC,
                name: String::from("FB_ID"),
                values: alloc::vec![u64::from(DRM_MODE_OBJECT_FB)],
                enums: Vec::new(),
            }),
            DRM_PROPERTY_ID_CRTC_ID | DRM_CONNECTOR_CRTC_ID_PROP_ID => Ok(DrmPropertyInfo {
                prop_id,
                flags: DRM_MODE_PROP_OBJECT | DRM_MODE_PROP_ATOMIC,
                name: String::from("CRTC_ID"),
                values: alloc::vec![u64::from(DRM_MODE_OBJECT_CRTC)],
                enums: Vec::new(),
            }),
            DRM_PROPERTY_ID_CRTC_X => Ok(DrmPropertyInfo {
                prop_id,
                flags: DRM_MODE_PROP_SIGNED_RANGE | DRM_MODE_PROP_ATOMIC,
                name: String::from("CRTC_X"),
                values: alloc::vec![u64::MAX.wrapping_sub(i32::MAX as u64 - 1), i32::MAX as u64],
                enums: Vec::new(),
            }),
            DRM_PROPERTY_ID_CRTC_Y => Ok(DrmPropertyInfo {
                prop_id,
                flags: DRM_MODE_PROP_SIGNED_RANGE | DRM_MODE_PROP_ATOMIC,
                name: String::from("CRTC_Y"),
                values: alloc::vec![u64::MAX.wrapping_sub(i32::MAX as u64 - 1), i32::MAX as u64],
                enums: Vec::new(),
            }),
            DRM_PROPERTY_ID_SRC_X => Ok(DrmPropertyInfo {
                prop_id,
                flags: DRM_MODE_PROP_RANGE | DRM_MODE_PROP_ATOMIC,
                name: String::from("SRC_X"),
                values: alloc::vec![0, u64::from(u32::MAX)],
                enums: Vec::new(),
            }),
            DRM_PROPERTY_ID_SRC_Y => Ok(DrmPropertyInfo {
                prop_id,
                flags: DRM_MODE_PROP_RANGE | DRM_MODE_PROP_ATOMIC,
                name: String::from("SRC_Y"),
                values: alloc::vec![0, u64::from(u32::MAX)],
                enums: Vec::new(),
            }),
            DRM_PROPERTY_ID_SRC_W => Ok(DrmPropertyInfo {
                prop_id,
                flags: DRM_MODE_PROP_RANGE | DRM_MODE_PROP_ATOMIC,
                name: String::from("SRC_W"),
                values: alloc::vec![0, u64::from(u32::MAX)],
                enums: Vec::new(),
            }),
            DRM_PROPERTY_ID_SRC_H => Ok(DrmPropertyInfo {
                prop_id,
                flags: DRM_MODE_PROP_RANGE | DRM_MODE_PROP_ATOMIC,
                name: String::from("SRC_H"),
                values: alloc::vec![0, u64::from(u32::MAX)],
                enums: Vec::new(),
            }),
            DRM_PROPERTY_ID_CRTC_W => Ok(DrmPropertyInfo {
                prop_id,
                flags: DRM_MODE_PROP_RANGE | DRM_MODE_PROP_ATOMIC,
                name: String::from("CRTC_W"),
                values: alloc::vec![0, 8192],
                enums: Vec::new(),
            }),
            DRM_PROPERTY_ID_CRTC_H => Ok(DrmPropertyInfo {
                prop_id,
                flags: DRM_MODE_PROP_RANGE | DRM_MODE_PROP_ATOMIC,
                name: String::from("CRTC_H"),
                values: alloc::vec![0, 8192],
                enums: Vec::new(),
            }),
            DRM_PROPERTY_ID_IN_FORMATS => Ok(DrmPropertyInfo {
                prop_id,
                flags: DRM_MODE_PROP_BLOB | DRM_MODE_PROP_IMMUTABLE,
                name: String::from("IN_FORMATS"),
                values: Vec::new(),
                enums: Vec::new(),
            }),
            DRM_CRTC_MODE_ID_PROP_ID => Ok(DrmPropertyInfo {
                prop_id,
                flags: DRM_MODE_PROP_BLOB | DRM_MODE_PROP_ATOMIC,
                name: String::from("MODE_ID"),
                values: Vec::new(),
                enums: Vec::new(),
            }),
            DRM_PROPERTY_ID_PLANE_TYPE => Ok(DrmPropertyInfo {
                prop_id,
                flags: DRM_MODE_PROP_ENUM | DRM_MODE_PROP_IMMUTABLE,
                name: String::from("type"),
                values: Vec::new(),
                enums: alloc::vec![
                    DrmPropertyEnumValue {
                        value: DRM_PLANE_TYPE_PRIMARY,
                        name: String::from("Primary"),
                    },
                    DrmPropertyEnumValue {
                        value: DRM_PLANE_TYPE_OVERLAY,
                        name: String::from("Overlay"),
                    },
                    DrmPropertyEnumValue {
                        value: DRM_PLANE_TYPE_CURSOR,
                        name: String::from("Cursor"),
                    },
                ],
            }),
            DRM_CRTC_ACTIVE_PROP_ID => Ok(DrmPropertyInfo {
                prop_id,
                flags: DRM_MODE_PROP_RANGE | DRM_MODE_PROP_ATOMIC,
                name: String::from("ACTIVE"),
                values: alloc::vec![0, 1],
                enums: Vec::new(),
            }),
            DRM_CONNECTOR_DPMS_PROP_ID => Ok(DrmPropertyInfo {
                prop_id,
                flags: DRM_MODE_PROP_ENUM,
                name: String::from("DPMS"),
                values: Vec::new(),
                enums: alloc::vec![
                    DrmPropertyEnumValue {
                        value: DRM_MODE_DPMS_ON,
                        name: String::from("On"),
                    },
                    DrmPropertyEnumValue {
                        value: DRM_MODE_DPMS_STANDBY,
                        name: String::from("Standby"),
                    },
                    DrmPropertyEnumValue {
                        value: DRM_MODE_DPMS_SUSPEND,
                        name: String::from("Suspend"),
                    },
                    DrmPropertyEnumValue {
                        value: DRM_MODE_DPMS_OFF,
                        name: String::from("Off"),
                    },
                ],
            }),
            DRM_CONNECTOR_EDID_PROP_ID => Ok(DrmPropertyInfo {
                prop_id,
                flags: DRM_MODE_PROP_BLOB | DRM_MODE_PROP_IMMUTABLE,
                name: String::from("EDID"),
                values: Vec::new(),
                enums: Vec::new(),
            }),
            DRM_FB_WIDTH_PROP_ID => Ok(DrmPropertyInfo {
                prop_id,
                flags: DRM_MODE_PROP_RANGE | DRM_MODE_PROP_ATOMIC,
                name: String::from("WIDTH"),
                values: alloc::vec![1, 8192],
                enums: Vec::new(),
            }),
            DRM_FB_HEIGHT_PROP_ID => Ok(DrmPropertyInfo {
                prop_id,
                flags: DRM_MODE_PROP_RANGE | DRM_MODE_PROP_ATOMIC,
                name: String::from("HEIGHT"),
                values: alloc::vec![1, 8192],
                enums: Vec::new(),
            }),
            DRM_FB_BPP_PROP_ID => Ok(DrmPropertyInfo {
                prop_id,
                flags: DRM_MODE_PROP_RANGE | DRM_MODE_PROP_ATOMIC,
                name: String::from("BPP"),
                values: alloc::vec![8, 32],
                enums: Vec::new(),
            }),
            DRM_FB_DEPTH_PROP_ID => Ok(DrmPropertyInfo {
                prop_id,
                flags: DRM_MODE_PROP_RANGE | DRM_MODE_PROP_ATOMIC,
                name: String::from("DEPTH"),
                values: alloc::vec![8, 32],
                enums: Vec::new(),
            }),
            _ => Err(DrmIoctlError::NotFound),
        }
    }

    pub fn atomic_commit(
        &self,
        flags: u32,
        obj_ids: &[u32],
        obj_prop_counts: &[u32],
        prop_ids: &[u32],
        prop_values: &[u64],
        user_data: u64,
    ) -> Result<(), DrmIoctlError> {
        if (flags & !super::ioctl::DRM_MODE_ATOMIC_FLAGS) != 0 {
            return Err(DrmIoctlError::Invalid);
        }
        if obj_ids.len() != obj_prop_counts.len() || prop_ids.len() != prop_values.len() {
            return Err(DrmIoctlError::Invalid);
        }

        let (
            mut next_fb_id,
            mut next_plane_crtc_id,
            mut next_connector_crtc_id,
            mut next_crtc_x,
            mut next_crtc_y,
            mut next_crtc_w,
            mut next_crtc_h,
            mut next_crtc_mode_valid,
            mut next_crtc_mode,
        ) = {
            let state = self.state.lock();
            (
                state.current_fb_id,
                state.plane_crtc_id,
                state.connector_crtc_id,
                state.crtc_x,
                state.crtc_y,
                state.crtc_w,
                state.crtc_h,
                state.crtc_mode_valid,
                state.crtc_mode,
            )
        };
        let mut prop_index = 0usize;
        let mut plane_fb_changed = false;

        for (&obj_id, &count) in obj_ids.iter().zip(obj_prop_counts.iter()) {
            let count = count as usize;
            if prop_index.saturating_add(count) > prop_ids.len() {
                return Err(DrmIoctlError::Invalid);
            }

            if obj_id == self.plane_id {
                for (&prop_id, &value) in prop_ids[prop_index..prop_index + count]
                    .iter()
                    .zip(&prop_values[prop_index..prop_index + count])
                {
                    match prop_id {
                        DRM_PROPERTY_ID_PLANE_TYPE => {
                            if value != DRM_PLANE_TYPE_PRIMARY {
                                return Err(DrmIoctlError::Invalid);
                            }
                        }
                        DRM_PROPERTY_ID_FB_ID => {
                            if value != 0 && self.get_framebuffer(value as u32).is_none() {
                                return Err(DrmIoctlError::NotFound);
                            }
                            next_fb_id = value as u32;
                            plane_fb_changed = true;
                        }
                        DRM_PROPERTY_ID_CRTC_ID => {
                            next_plane_crtc_id = value as u32;
                        }
                        DRM_PROPERTY_ID_SRC_X
                        | DRM_PROPERTY_ID_SRC_Y
                        | DRM_PROPERTY_ID_SRC_W
                        | DRM_PROPERTY_ID_SRC_H
                        | DRM_PROPERTY_ID_IN_FORMATS => {}
                        DRM_PROPERTY_ID_CRTC_X => {
                            next_crtc_x = value as u32;
                        }
                        DRM_PROPERTY_ID_CRTC_Y => {
                            next_crtc_y = value as u32;
                        }
                        DRM_PROPERTY_ID_CRTC_W => {
                            next_crtc_w = value as u32;
                        }
                        DRM_PROPERTY_ID_CRTC_H => {
                            next_crtc_h = value as u32;
                        }
                        _ => {}
                    }
                }
            } else if obj_id == self.crtc_id {
                for (&prop_id, &value) in prop_ids[prop_index..prop_index + count]
                    .iter()
                    .zip(&prop_values[prop_index..prop_index + count])
                {
                    match prop_id {
                        DRM_CRTC_ACTIVE_PROP_ID => next_crtc_mode_valid = value != 0,
                        DRM_CRTC_MODE_ID_PROP_ID => {
                            if value == 0 {
                                next_crtc_mode_valid = false;
                                next_crtc_mode = DrmModeInfo::simple(0, 0, 0);
                                next_crtc_w = 0;
                                next_crtc_h = 0;
                            } else {
                                let mode = self.mode_from_blob(value as u32)?;
                                if mode.hdisplay == 0 || mode.vdisplay == 0 {
                                    return Err(DrmIoctlError::Invalid);
                                }
                                next_crtc_mode = mode;
                                next_crtc_mode_valid = true;
                                next_crtc_w = u32::from(mode.hdisplay);
                                next_crtc_h = u32::from(mode.vdisplay);
                            }
                        }
                        _ => {}
                    }
                }
            } else if obj_id == self.connector_id {
                for (&prop_id, &value) in prop_ids[prop_index..prop_index + count]
                    .iter()
                    .zip(&prop_values[prop_index..prop_index + count])
                {
                    match prop_id {
                        DRM_CONNECTOR_DPMS_PROP_ID => {
                            if value > DRM_MODE_DPMS_OFF {
                                return Err(DrmIoctlError::Invalid);
                            }
                        }
                        DRM_CONNECTOR_CRTC_ID_PROP_ID => next_connector_crtc_id = value as u32,
                        DRM_CONNECTOR_EDID_PROP_ID => {}
                        _ => {}
                    }
                }
            } else if self.get_framebuffer(obj_id).is_some() {
            } else {
                return Err(DrmIoctlError::NotFound);
            }

            prop_index += count;
        }

        if prop_index != prop_ids.len() {
            return Err(DrmIoctlError::Invalid);
        }
        if next_plane_crtc_id != 0 && next_plane_crtc_id != self.crtc_id {
            return Err(DrmIoctlError::NotFound);
        }
        if next_connector_crtc_id != 0 && next_connector_crtc_id != self.crtc_id {
            return Err(DrmIoctlError::NotFound);
        }
        if (flags & super::ioctl::DRM_MODE_ATOMIC_TEST_ONLY) != 0 {
            return Ok(());
        }

        let mut state = self.state.lock();
        state.current_fb_id = next_fb_id;
        state.plane_crtc_id = next_plane_crtc_id;
        state.connector_crtc_id = next_connector_crtc_id;
        state.crtc_x = next_crtc_x;
        state.crtc_y = next_crtc_y;
        state.crtc_w = next_crtc_w;
        state.crtc_h = next_crtc_h;
        state.crtc_mode_valid = next_crtc_mode_valid;
        state.crtc_mode = next_crtc_mode;
        drop(state);

        if plane_fb_changed && next_crtc_mode_valid && next_fb_id != 0 {
            self.present_framebuffer(next_fb_id)?;
            self.bump();
        }
        if (flags & DRM_MODE_PAGE_FLIP_EVENT) != 0 {
            self.defer_flip_event(user_data);
        }
        Ok(())
    }

    pub fn create_dumb(
        &self,
        width: u32,
        height: u32,
        bpp: u32,
        flags: u32,
    ) -> Result<(u32, u32, u64), DrmIoctlError> {
        if width == 0 || height == 0 || flags != 0 {
            return Err(DrmIoctlError::Invalid);
        }
        let bpp = if bpp == 0 { 32 } else { bpp };
        let bytes_per_pixel = bpp.div_ceil(8);
        if bytes_per_pixel == 0 {
            return Err(DrmIoctlError::Invalid);
        }

        let pitch = (width.saturating_mul(bytes_per_pixel).saturating_add(63)) & !63;
        let size = height.saturating_mul(pitch) as usize;
        let dma = DmaRegion::new(size).map_err(|_| DrmIoctlError::NoMemory)?;
        let mut state = self.state.lock();
        let handle = state.next_handle;
        state.next_handle = state.next_handle.saturating_add(1);
        state.dumb_buffers.insert(
            handle,
            Arc::new(DumbBuffer {
                handle,
                width,
                height,
                bpp,
                pitch,
                size,
                dma,
            }),
        );
        self.bump();
        Ok((handle, pitch, size as u64))
    }

    pub fn map_dumb(&self, handle: u32) -> Result<u64, DrmIoctlError> {
        let state = self.state.lock();
        state
            .dumb_buffers
            .get(&handle)
            .map(|dumb| dumb.phys_addr())
            .ok_or(DrmIoctlError::NotFound)
    }

    pub fn destroy_dumb(&self, handle: u32) -> Result<(), DrmIoctlError> {
        let mut state = self.state.lock();
        if state.current_fb_id != 0
            && state
                .framebuffers
                .get(&state.current_fb_id)
                .is_some_and(|fb| fb.handle == handle)
        {
            return Err(DrmIoctlError::Busy);
        }
        if state.framebuffers.values().any(|fb| fb.handle == handle) {
            return Err(DrmIoctlError::Busy);
        }
        if state.dumb_buffers.remove(&handle).is_none() {
            return Err(DrmIoctlError::NotFound);
        }
        self.bump();
        Ok(())
    }

    pub fn add_framebuffer2(&self, create: DrmFramebufferCreate) -> Result<u32, DrmIoctlError> {
        if create.width == 0 || create.height == 0 || create.handle == 0 {
            return Err(DrmIoctlError::Invalid);
        }
        if create.flags != 0 {
            return Err(DrmIoctlError::Invalid);
        }

        let mut state = self.state.lock();
        let dumb = state
            .dumb_buffers
            .get(&create.handle)
            .ok_or(DrmIoctlError::NotFound)?;
        if !self
            .backend
            .supported_formats()
            .contains(&create.pixel_format)
        {
            return Err(DrmIoctlError::NotSupported);
        }
        let pitch = if create.pitch == 0 {
            dumb.pitch
        } else {
            create.pitch
        };
        let fb_id = state.next_fb_id;
        state.next_fb_id = state.next_fb_id.saturating_add(1);
        state.framebuffers.insert(
            fb_id,
            DrmFramebufferState {
                id: fb_id,
                width: create.width,
                height: create.height,
                pitch,
                depth: 24,
                bpp: 32,
                handle: create.handle,
                pixel_format: create.pixel_format,
            },
        );
        self.bump();
        Ok(fb_id)
    }

    pub fn remove_framebuffer(&self, framebuffer_id: u32) -> Result<(), DrmIoctlError> {
        let mut state = self.state.lock();
        if framebuffer_id == state.current_fb_id {
            return Err(DrmIoctlError::Busy);
        }
        if state.framebuffers.remove(&framebuffer_id).is_none() {
            return Err(DrmIoctlError::NotFound);
        }
        self.bump();
        Ok(())
    }

    pub fn set_crtc(&self, framebuffer_id: u32) -> Result<(), DrmIoctlError> {
        self.present_framebuffer(framebuffer_id)?;
        let mut state = self.state.lock();
        state.current_fb_id = framebuffer_id;
        state.plane_crtc_id = self.crtc_id;
        state.connector_crtc_id = self.crtc_id;
        self.bump();
        Ok(())
    }

    pub fn dirty_framebuffer(&self, framebuffer_id: u32, flags: u32) -> Result<(), DrmIoctlError> {
        if (flags & !DRM_MODE_FB_DIRTY_FLAGS) != 0 {
            return Err(DrmIoctlError::Invalid);
        }
        self.present_framebuffer(framebuffer_id)?;
        self.bump();
        Ok(())
    }

    fn present_framebuffer(&self, framebuffer_id: u32) -> Result<(), DrmIoctlError> {
        let (dumb, width, height, pitch, format) = {
            let state = self.state.lock();
            let framebuffer = state
                .framebuffers
                .get(&framebuffer_id)
                .ok_or(DrmIoctlError::NotFound)?;
            let dumb = state
                .dumb_buffers
                .get(&framebuffer.handle)
                .ok_or(DrmIoctlError::NotFound)?;
            (
                dumb.clone(),
                framebuffer.width,
                framebuffer.height,
                framebuffer.pitch,
                framebuffer.pixel_format,
            )
        };

        self.backend
            .present(dumb.bytes(), width, height, pitch, format)?;
        Ok(())
    }

    pub fn page_flip(
        &self,
        framebuffer_id: u32,
        flags: u32,
        user_data: u64,
    ) -> Result<(), DrmIoctlError> {
        if (flags & !DRM_MODE_PAGE_FLIP_FLAGS) != 0 {
            return Err(DrmIoctlError::Invalid);
        }
        self.set_crtc(framebuffer_id)?;
        if (flags & DRM_MODE_PAGE_FLIP_EVENT) != 0 {
            self.defer_flip_event(user_data);
        }
        Ok(())
    }

    pub fn wait_vblank(
        &self,
        request: super::ioctl::DrmWaitVBlank,
    ) -> Result<super::ioctl::DrmWaitVBlank, DrmIoctlError> {
        let supported = super::ioctl::DRM_VBLANK_TYPES_MASK
            | super::ioctl::DRM_VBLANK_FLAGS_MASK
            | super::ioctl::DRM_VBLANK_HIGH_CRTC_MASK;
        if (request.type_ & !supported) != 0 {
            return Err(DrmIoctlError::Invalid);
        }
        if (request.type_ & super::ioctl::DRM_VBLANK_SIGNAL) != 0 {
            return Err(DrmIoctlError::Invalid);
        }
        if (request.type_ & super::ioctl::DRM_VBLANK_TYPES_MASK) > super::ioctl::DRM_VBLANK_RELATIVE
        {
            return Err(DrmIoctlError::Invalid);
        }
        let high_crtc = (request.type_ & super::ioctl::DRM_VBLANK_HIGH_CRTC_MASK)
            >> super::ioctl::DRM_VBLANK_HIGH_CRTC_SHIFT;
        if high_crtc != 0
            || (request.type_ & super::ioctl::DRM_VBLANK_SECONDARY) != 0
            || (request.type_ & super::ioctl::DRM_VBLANK_FLIP) != 0
        {
            return Err(DrmIoctlError::NotSupported);
        }

        let mut target_set = false;
        let mut target_seq = 0u64;

        loop {
            let now_ns = time::monotonic_nanos();
            let (sequence, next_vblank_ns) = {
                let mut state = self.state.lock();
                Self::advance_vblank_state_locked(&mut state, now_ns);
                (state.vblank_sequence, state.next_vblank_ns)
            };

            if !target_set {
                target_seq = if (request.type_ & super::ioctl::DRM_VBLANK_RELATIVE) != 0 {
                    sequence.saturating_add(request.sequence as u64)
                } else {
                    request.sequence as u64
                };
                if (request.type_ & super::ioctl::DRM_VBLANK_NEXTONMISS) != 0
                    && sequence >= target_seq
                {
                    target_seq = sequence.saturating_add(1);
                }
                target_set = true;
            }

            if (request.type_ & super::ioctl::DRM_VBLANK_EVENT) != 0 {
                let notify_now = {
                    let mut state = self.state.lock();
                    Self::advance_vblank_state_locked(&mut state, now_ns);
                    if state.vblank_sequence >= target_seq || state.vblank_period_ns == 0 {
                        let sequence = state.vblank_sequence as u32;
                        self.queue_ready_event_locked(
                            &mut state,
                            DRM_EVENT_VBLANK,
                            request.signal,
                            now_ns,
                            sequence,
                        );
                        true
                    } else {
                        if state.pending_events.len() >= MAX_PENDING_DRM_EVENTS {
                            let _ = state.pending_events.pop_front();
                        }
                        state.pending_events.push_back(PendingDrmEvent {
                            type_: DRM_EVENT_VBLANK,
                            user_data: request.signal,
                            target_sequence: target_seq,
                        });
                        false
                    }
                };
                self.bump();
                refresh_next_vblank_deadline();
                if notify_now {
                    self.waiters.notify(PollEvents::READ);
                }
                return Ok(request);
            }

            if sequence >= target_seq {
                return Ok(super::ioctl::DrmWaitVBlank {
                    type_: request.type_,
                    sequence: sequence as u32,
                    signal: 0,
                    tval_sec: (now_ns / 1_000_000_000) as i64,
                    tval_usec: ((now_ns % 1_000_000_000) / 1_000) as i64,
                });
            }

            let wait_ns = if next_vblank_ns > now_ns {
                (next_vblank_ns - now_ns).min(10_000_000)
            } else {
                1_000_000
            };
            if wait_ns == 0 {
                continue;
            }
            let _ = time::spin_delay_nanos(wait_ns);
        }
    }

    fn drop_oldest_ready_event_locked(state: &mut DrmState) {
        let _ = state.event_queue.pop_front();
    }

    fn queue_ready_event_locked(
        &self,
        state: &mut DrmState,
        type_: u32,
        user_data: u64,
        monotonic_ns: u64,
        sequence: u32,
    ) {
        let tv_sec = (monotonic_ns / 1_000_000_000) as u32;
        let tv_usec = ((monotonic_ns % 1_000_000_000) / 1_000) as u32;
        let mut event = [0u8; DRM_EVENT_BYTES];
        let event_len = event.len() as u32;
        event[0..4].copy_from_slice(&type_.to_ne_bytes());
        event[4..8].copy_from_slice(&event_len.to_ne_bytes());
        event[8..16].copy_from_slice(&user_data.to_ne_bytes());
        event[16..20].copy_from_slice(&tv_sec.to_ne_bytes());
        event[20..24].copy_from_slice(&tv_usec.to_ne_bytes());
        event[24..28].copy_from_slice(&sequence.to_ne_bytes());
        event[28..32].copy_from_slice(&self.crtc_id.to_ne_bytes());

        while state.event_queue.len() >= MAX_READY_DRM_EVENTS {
            Self::drop_oldest_ready_event_locked(state);
        }
        state.event_queue.push_back(event);
    }

    fn defer_flip_event(&self, user_data: u64) {
        let now_ns = time::monotonic_nanos();
        let notify_now = {
            let mut state = self.state.lock();
            if state.vblank_period_ns == 0 {
                state.vblank_sequence = state.vblank_sequence.saturating_add(1);
                let sequence = state.vblank_sequence as u32;
                self.queue_ready_event_locked(
                    &mut state,
                    DRM_EVENT_FLIP_COMPLETE,
                    user_data,
                    now_ns,
                    sequence,
                );
                true
            } else {
                Self::advance_vblank_state_locked(&mut state, now_ns);
                let target_sequence = state.vblank_sequence.saturating_add(1);
                if state.pending_events.len() >= MAX_PENDING_DRM_EVENTS {
                    let _ = state.pending_events.pop_front();
                }
                state.pending_events.push_back(PendingDrmEvent {
                    type_: DRM_EVENT_FLIP_COMPLETE,
                    user_data,
                    target_sequence,
                });
                false
            }
        };
        self.bump();
        refresh_next_vblank_deadline();
        if notify_now {
            self.waiters.notify(PollEvents::READ);
        }
    }

    fn drain_events(&self, buffer: &mut [u8]) -> FsResult<usize> {
        let mut state = self.state.lock();
        if state.event_queue.is_empty() {
            return Err(FsError::WouldBlock);
        }
        if buffer.len() < DRM_EVENT_BYTES {
            return Err(FsError::InvalidInput);
        }
        let mut written = 0usize;
        while written + DRM_EVENT_BYTES <= buffer.len() {
            let Some(event) = state.event_queue.pop_front() else {
                break;
            };
            buffer[written..written + DRM_EVENT_BYTES].copy_from_slice(&event);
            written += DRM_EVENT_BYTES;
        }
        Ok(written)
    }

    fn has_events(&self) -> bool {
        !self.state.lock().event_queue.is_empty()
    }

    fn handle_vblank_tick(&self, now_ns: u64) {
        let notify = {
            let mut state = self.state.lock();
            if state.vblank_period_ns == 0 {
                return;
            }

            let previous_sequence = state.vblank_sequence;
            Self::advance_vblank_state_locked(&mut state, now_ns);
            if state.vblank_sequence == previous_sequence {
                return;
            }
            if state.pending_events.is_empty() {
                return;
            }

            let mut queued = false;
            let current_sequence = state.vblank_sequence;
            let mut deferred = VecDeque::new();
            while let Some(event) = state.pending_events.pop_front() {
                if event.target_sequence <= current_sequence {
                    self.queue_ready_event_locked(
                        &mut state,
                        event.type_,
                        event.user_data,
                        now_ns,
                        current_sequence as u32,
                    );
                    queued = true;
                } else {
                    deferred.push_back(event);
                }
            }
            state.pending_events = deferred;

            queued
        };

        refresh_next_vblank_deadline();
        if notify {
            self.bump();
            self.waiters.notify(PollEvents::READ);
        }
    }

    fn advance_vblank_state_locked(state: &mut DrmState, now_ns: u64) {
        if state.vblank_period_ns == 0 {
            return;
        }
        if state.next_vblank_ns == 0 {
            state.next_vblank_ns = now_ns.saturating_add(state.vblank_period_ns);
            return;
        }
        if now_ns < state.next_vblank_ns {
            return;
        }

        let periods = now_ns.saturating_sub(state.next_vblank_ns) / state.vblank_period_ns + 1;
        state.vblank_sequence = state.vblank_sequence.saturating_add(periods);
        state.next_vblank_ns = state
            .next_vblank_ns
            .saturating_add(periods.saturating_mul(state.vblank_period_ns));
    }

    fn mmap_dumb(&self, offset: u64, length: u64) -> FsResult<MmapResponse> {
        let state = self.state.lock();
        let dumb = state
            .dumb_buffers
            .values()
            .find(|dumb| dumb.phys_addr() == offset)
            .ok_or(FsError::InvalidInput)?;
        if length > dumb.size as u64 {
            return Err(FsError::InvalidInput);
        }
        Ok(MmapResponse::direct_physical(
            dumb.phys_addr(),
            MmapCachePolicy::Cached,
        ))
    }
}

pub struct DrmFile {
    device: Arc<DrmDevice>,
}

impl DrmFile {
    pub fn new(device: Arc<DrmDevice>) -> Arc<Self> {
        Arc::new(Self { device })
    }

    pub fn device(&self) -> &Arc<DrmDevice> {
        &self.device
    }
}

impl FileOperations for DrmFile {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn read(&self, _offset: usize, buffer: &mut [u8]) -> FsResult<usize> {
        self.device.drain_events(buffer)
    }

    fn poll(&self, events: PollEvents) -> FsResult<PollEvents> {
        let mut ready = PollEvents::empty();
        if events.contains(PollEvents::READ) && self.device.has_events() {
            ready = ready | PollEvents::READ;
        }
        Ok(ready)
    }

    fn wait_token(&self) -> u64 {
        self.device.version()
    }

    fn register_waiter(
        &self,
        events: PollEvents,
        listener: SharedWaitListener,
    ) -> FsResult<Option<u64>> {
        Ok(Some(self.device.waiters.register(events, listener)))
    }

    fn unregister_waiter(&self, waiter_id: u64) -> FsResult<()> {
        let _ = self.device.waiters.unregister(waiter_id);
        Ok(())
    }

    fn mmap(&self, request: MmapRequest) -> FsResult<MmapResponse> {
        self.device.mmap_dumb(request.offset, request.length)
    }
}

struct DrmPrimaryDeviceNode {
    name: String,
    minor: u16,
    file: Arc<DrmFile>,
}

impl KernelDevice for DrmPrimaryDeviceNode {
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn metadata(&self) -> DeviceMetadata {
        DeviceMetadata::new(self.name.clone(), DeviceClass::Drm, DRM_MAJOR, self.minor)
    }

    fn nodes(&self) -> Vec<DeviceNode> {
        let node: NodeRef = FileNode::new_char_device(
            self.name.clone(),
            u32::from(DRM_MAJOR),
            u32::from(self.minor),
            self.file.clone(),
        );
        alloc::vec![DeviceNode::new(alloc::format!("dri/{}", self.name), node)]
    }
}

pub fn probe(registry: &mut DeviceRegistry) {
    if let Some(info) = aether_frame::boot::info().framebuffer
        && let Ok(surface) = FramebufferSurface::from_boot_info(info)
    {
        let backend: Arc<dyn DrmScanoutBackend> = Arc::new(PlainFbBackend::new(surface));
        let device = DrmDevice::new(0, backend);
        let primary = DrmFile::new(device);
        registry.register(Arc::new(DrmPrimaryDeviceNode {
            name: "card0".to_string(),
            minor: 0,
            file: primary,
        }));
    }
}

pub fn handle_vblank_tick() {
    let now_ns = time::monotonic_nanos();
    let devices: Vec<_> = DRM_DEVICES
        .lock()
        .iter()
        .filter_map(Weak::upgrade)
        .collect();
    for device in devices {
        device.handle_vblank_tick(now_ns);
    }
}

pub fn vblank_deadline_due() -> bool {
    let deadline = NEXT_VBLANK_DEADLINE_NS.load(Ordering::Acquire);
    deadline != u64::MAX && time::monotonic_nanos() >= deadline
}

pub fn next_vblank_wakeup_deadline() -> Option<u64> {
    let deadline = NEXT_VBLANK_DEADLINE_NS.load(Ordering::Acquire);
    (deadline != u64::MAX).then_some(deadline)
}

fn refresh_next_vblank_deadline() {
    let mut next_deadline = u64::MAX;
    let mut devices = DRM_DEVICES.lock();
    devices.retain(|weak| {
        let Some(device) = weak.upgrade() else {
            return false;
        };
        let deadline = {
            let state = device.state.lock();
            (state.vblank_period_ns != 0 && !state.pending_events.is_empty())
                .then_some(state.next_vblank_ns)
        };
        if let Some(deadline) = deadline {
            next_deadline = next_deadline.min(deadline);
        }
        true
    });
    NEXT_VBLANK_DEADLINE_NS.store(next_deadline, Ordering::Release);
}

pub(crate) fn decode_argb(pixel_format: u32, pixel: u32) -> Option<(u8, u8, u8)> {
    let (red, green, blue) = match pixel_format {
        DRM_FORMAT_XRGB8888 | DRM_FORMAT_ARGB8888 => (
            ((pixel >> 16) & 0xff) as u8,
            ((pixel >> 8) & 0xff) as u8,
            (pixel & 0xff) as u8,
        ),
        DRM_FORMAT_XBGR8888 | DRM_FORMAT_ABGR8888 => (
            (pixel & 0xff) as u8,
            ((pixel >> 8) & 0xff) as u8,
            ((pixel >> 16) & 0xff) as u8,
        ),
        _ => return None,
    };
    Some((red, green, blue))
}

pub(crate) fn color_from_pixel(pixel_format: u32, pixel: u32) -> Option<RgbColor> {
    decode_argb(pixel_format, pixel).map(|(red, green, blue)| RgbColor { red, green, blue })
}
