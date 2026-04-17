extern crate alloc;

use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;
use core::sync::atomic::{AtomicU64, Ordering};

use aether_device::{DeviceClass, DeviceMetadata, DeviceNode, DeviceRegistry, KernelDevice};
use aether_frame::interrupt::timer;
use aether_frame::libs::spin::SpinLock;
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
pub const DRM_MODE_OBJECT_FB: u32 = 0xfbfb_fbfb;
pub const DRM_MODE_OBJECT_PLANE: u32 = 0xeeee_eeee;
pub const DRM_MODE_OBJECT_ANY: u32 = 0;

pub const DRM_MODE_PROP_RANGE: u32 = 1 << 1;
pub const DRM_MODE_PROP_IMMUTABLE: u32 = 1 << 2;
pub const DRM_MODE_PROP_ENUM: u32 = 1 << 3;
pub const DRM_MODE_PROP_OBJECT: u32 = 1 << 6;
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
pub const DRM_CRTC_ACTIVE_PROP_ID: u32 = 0x100;
pub const DRM_CONNECTOR_DPMS_PROP_ID: u32 = 0x200;
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
        Self {
            clock: width.saturating_mul(refresh_hz),
            hdisplay: width as u16,
            hsync_start: width.saturating_add(16) as u16,
            hsync_end: width.saturating_add(16 + 96) as u16,
            htotal: width.saturating_add(16 + 96 + 48) as u16,
            hskew: 0,
            vdisplay: height as u16,
            vsync_start: height.saturating_add(10) as u16,
            vsync_end: height.saturating_add(12) as u16,
            vtotal: height.saturating_add(45) as u16,
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

struct DrmState {
    next_handle: u32,
    next_fb_id: u32,
    current_fb_id: u32,
    master_pid: Option<u32>,
    universal_planes: bool,
    dumb_buffers: BTreeMap<u32, DumbBuffer>,
    framebuffers: BTreeMap<u32, DrmFramebufferState>,
    event_queue: VecDeque<u8>,
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
    pub fn new(index: usize, backend: Arc<dyn DrmScanoutBackend>) -> Arc<Self> {
        Arc::new(Self {
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
                master_pid: None,
                universal_planes: false,
                dumb_buffers: BTreeMap::new(),
                framebuffers: BTreeMap::new(),
                event_queue: VecDeque::new(),
            }),
        })
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
                self.state.lock_irqsave().universal_planes = value != 0;
                Ok(())
            }
            DRM_CLIENT_CAP_ATOMIC => {
                if value == 0 {
                    return Ok(());
                }
                // TODO: expose atomic once per-file drm client state exists.
                Err(DrmIoctlError::NotSupported)
            }
            _ => Err(DrmIoctlError::Invalid),
        }
    }

    pub fn set_master(&self, pid: u32) -> Result<(), DrmIoctlError> {
        let mut state = self.state.lock_irqsave();
        match state.master_pid {
            Some(owner) if owner != pid => Err(DrmIoctlError::Busy),
            _ => {
                state.master_pid = Some(pid);
                Ok(())
            }
        }
    }

    pub fn drop_master(&self, pid: u32) -> Result<(), DrmIoctlError> {
        let mut state = self.state.lock_irqsave();
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
        let state = self.state.lock_irqsave();
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
            let state = self.state.lock_irqsave();
            DrmCrtcSnapshot {
                crtc_id: self.crtc_id,
                framebuffer_id: state.current_fb_id,
                x: 0,
                y: 0,
                gamma_size: 0,
                mode_valid: true,
                mode: self.backend.mode(),
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
        let _ = self.state.lock_irqsave().universal_planes;
        alloc::vec![self.plane_id]
    }

    pub fn get_plane(&self, plane_id: u32) -> Option<DrmPlaneSnapshot> {
        (plane_id == self.plane_id).then(|| {
            let state = self.state.lock_irqsave();
            DrmPlaneSnapshot {
                plane_id: self.plane_id,
                crtc_id: self.crtc_id,
                framebuffer_id: state.current_fb_id,
                possible_crtcs: 1,
                gamma_size: 0,
                format_types: self.backend.supported_formats().to_vec(),
            }
        })
    }

    pub fn get_framebuffer(&self, framebuffer_id: u32) -> Option<DrmFramebufferSnapshot> {
        let state = self.state.lock_irqsave();
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
                Ok(DrmObjectPropertiesSnapshot {
                    ids: alloc::vec![DRM_CRTC_ACTIVE_PROP_ID],
                    values: alloc::vec![1],
                })
            }
            DRM_MODE_OBJECT_CONNECTOR => {
                if object_id != self.connector_id {
                    return Err(DrmIoctlError::NotFound);
                }
                Ok(DrmObjectPropertiesSnapshot {
                    ids: alloc::vec![DRM_CONNECTOR_DPMS_PROP_ID, DRM_CONNECTOR_CRTC_ID_PROP_ID],
                    values: alloc::vec![DRM_MODE_DPMS_ON, u64::from(self.crtc_id)],
                })
            }
            DRM_MODE_OBJECT_PLANE => {
                let snapshot = self.get_plane(object_id).ok_or(DrmIoctlError::NotFound)?;
                Ok(DrmObjectPropertiesSnapshot {
                    ids: alloc::vec![
                        DRM_PROPERTY_ID_PLANE_TYPE,
                        DRM_PROPERTY_ID_FB_ID,
                        DRM_PROPERTY_ID_CRTC_ID,
                        DRM_PROPERTY_ID_CRTC_X,
                        DRM_PROPERTY_ID_CRTC_Y,
                    ],
                    values: alloc::vec![
                        DRM_PLANE_TYPE_PRIMARY,
                        u64::from(snapshot.framebuffer_id),
                        u64::from(snapshot.crtc_id),
                        0,
                        0,
                    ],
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
                flags: DRM_MODE_PROP_RANGE | DRM_MODE_PROP_ATOMIC,
                name: String::from("CRTC_X"),
                values: alloc::vec![0, u64::from(self.backend.mode().hdisplay)],
                enums: Vec::new(),
            }),
            DRM_PROPERTY_ID_CRTC_Y => Ok(DrmPropertyInfo {
                prop_id,
                flags: DRM_MODE_PROP_RANGE | DRM_MODE_PROP_ATOMIC,
                name: String::from("CRTC_Y"),
                values: alloc::vec![0, u64::from(self.backend.mode().vdisplay)],
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
        let mut state = self.state.lock_irqsave();
        let handle = state.next_handle;
        state.next_handle = state.next_handle.saturating_add(1);
        state.dumb_buffers.insert(
            handle,
            DumbBuffer {
                handle,
                width,
                height,
                bpp,
                pitch,
                size,
                dma,
            },
        );
        self.bump();
        Ok((handle, pitch, size as u64))
    }

    pub fn map_dumb(&self, handle: u32) -> Result<u64, DrmIoctlError> {
        let state = self.state.lock_irqsave();
        state
            .dumb_buffers
            .get(&handle)
            .map(DumbBuffer::phys_addr)
            .ok_or(DrmIoctlError::NotFound)
    }

    pub fn destroy_dumb(&self, handle: u32) -> Result<(), DrmIoctlError> {
        let mut state = self.state.lock_irqsave();
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

        let mut state = self.state.lock_irqsave();
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
        let mut state = self.state.lock_irqsave();
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
        let (bytes, width, height, pitch, format) = {
            let state = self.state.lock_irqsave();
            let framebuffer = state
                .framebuffers
                .get(&framebuffer_id)
                .ok_or(DrmIoctlError::NotFound)?;
            let dumb = state
                .dumb_buffers
                .get(&framebuffer.handle)
                .ok_or(DrmIoctlError::NotFound)?;
            (
                dumb.bytes().to_vec(),
                framebuffer.width,
                framebuffer.height,
                framebuffer.pitch,
                framebuffer.pixel_format,
            )
        };

        self.backend
            .present(bytes.as_slice(), width, height, pitch, format)?;
        self.state.lock_irqsave().current_fb_id = framebuffer_id;
        self.bump();
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
            self.enqueue_flip_event(user_data);
        }
        Ok(())
    }

    fn enqueue_flip_event(&self, user_data: u64) {
        let monotonic_ns = timer::nanos_since_boot();
        let tv_sec = (monotonic_ns / 1_000_000_000) as u32;
        let tv_usec = ((monotonic_ns % 1_000_000_000) / 1_000) as u32;
        let mut event = [0u8; 32];
        let event_len = event.len() as u32;
        event[0..4].copy_from_slice(&DRM_EVENT_FLIP_COMPLETE.to_ne_bytes());
        event[4..8].copy_from_slice(&event_len.to_ne_bytes());
        event[8..16].copy_from_slice(&user_data.to_ne_bytes());
        event[16..20].copy_from_slice(&tv_sec.to_ne_bytes());
        event[20..24].copy_from_slice(&tv_usec.to_ne_bytes());
        event[24..28].copy_from_slice(&0u32.to_ne_bytes());
        event[28..32].copy_from_slice(&self.crtc_id.to_ne_bytes());

        let mut state = self.state.lock_irqsave();
        state.event_queue.extend(event);
        drop(state);
        self.bump();
        self.waiters.notify(PollEvents::READ);
    }

    fn drain_events(&self, buffer: &mut [u8]) -> FsResult<usize> {
        let mut state = self.state.lock_irqsave();
        if state.event_queue.is_empty() {
            return Err(FsError::WouldBlock);
        }
        let event_len = if state.event_queue.len() >= 8 {
            let mut len_bytes = [0u8; 4];
            len_bytes.copy_from_slice(
                state
                    .event_queue
                    .iter()
                    .skip(4)
                    .take(4)
                    .copied()
                    .collect::<Vec<_>>()
                    .as_slice(),
            );
            u32::from_ne_bytes(len_bytes) as usize
        } else {
            0
        };
        if event_len == 0 || buffer.len() < event_len {
            return Err(FsError::InvalidInput);
        }
        let mut written = 0usize;
        while written + event_len <= buffer.len() && state.event_queue.len() >= event_len {
            for byte in &mut buffer[written..written + event_len] {
                *byte = state
                    .event_queue
                    .pop_front()
                    .expect("event queue length checked");
            }
            written += event_len;
            if state.event_queue.len() < 8 {
                break;
            }
        }
        Ok(written)
    }

    fn has_events(&self) -> bool {
        !self.state.lock_irqsave().event_queue.is_empty()
    }

    fn mmap_dumb(&self, offset: u64, length: u64) -> FsResult<MmapResponse> {
        let state = self.state.lock_irqsave();
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
