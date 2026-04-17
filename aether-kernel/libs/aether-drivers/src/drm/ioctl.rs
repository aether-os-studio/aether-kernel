extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;

use super::core::{DrmModeInfo, DrmPropertyEnumValue};

const IOC_NRBITS: u32 = 8;
const IOC_TYPEBITS: u32 = 8;
const IOC_SIZEBITS: u32 = 14;
const IOC_DIRBITS: u32 = 2;

const IOC_NRSHIFT: u32 = 0;
const IOC_TYPESHIFT: u32 = IOC_NRSHIFT + IOC_NRBITS;
const IOC_SIZESHIFT: u32 = IOC_TYPESHIFT + IOC_TYPEBITS;
const IOC_DIRSHIFT: u32 = IOC_SIZESHIFT + IOC_SIZEBITS;

pub const IOC_NONE: u32 = 0;
pub const IOC_WRITE: u32 = 1;
pub const IOC_READ: u32 = 2;

const DRM_IOCTL_BASE: u8 = b'd';

const fn ioc(dir: u32, ty: u8, nr: u8, size: usize) -> u64 {
    ((dir as u64) << IOC_DIRSHIFT)
        | ((ty as u64) << IOC_TYPESHIFT)
        | ((nr as u64) << IOC_NRSHIFT)
        | ((size as u64) << IOC_SIZESHIFT)
}

pub const fn io(ty: u8, nr: u8) -> u64 {
    ioc(IOC_NONE, ty, nr, 0)
}

pub const fn iow(ty: u8, nr: u8, size: usize) -> u64 {
    ioc(IOC_WRITE, ty, nr, size)
}

pub const fn iowr(ty: u8, nr: u8, size: usize) -> u64 {
    ioc(IOC_READ | IOC_WRITE, ty, nr, size)
}

pub const fn ioctl_dir(command: u64) -> u32 {
    ((command >> IOC_DIRSHIFT) & ((1 << IOC_DIRBITS) - 1) as u64) as u32
}

pub const fn ioctl_size(command: u64) -> usize {
    ((command >> IOC_SIZESHIFT) & ((1 << IOC_SIZEBITS) - 1) as u64) as usize
}

pub const DRM_IOCTL_VERSION: u64 = iowr(DRM_IOCTL_BASE, 0x00, DrmVersion::SIZE);
pub const DRM_IOCTL_GET_CAP: u64 = iowr(DRM_IOCTL_BASE, 0x0c, DrmGetCap::SIZE);
pub const DRM_IOCTL_SET_CLIENT_CAP: u64 = iow(DRM_IOCTL_BASE, 0x0d, DrmSetClientCap::SIZE);
pub const DRM_IOCTL_SET_MASTER: u64 = io(DRM_IOCTL_BASE, 0x1e);
pub const DRM_IOCTL_DROP_MASTER: u64 = io(DRM_IOCTL_BASE, 0x1f);
pub const DRM_IOCTL_MODE_GETRESOURCES: u64 = iowr(DRM_IOCTL_BASE, 0xa0, DrmModeCardRes::SIZE);
pub const DRM_IOCTL_MODE_GETCRTC: u64 = iowr(DRM_IOCTL_BASE, 0xa1, DrmModeCrtc::SIZE);
pub const DRM_IOCTL_MODE_GETENCODER: u64 = iowr(DRM_IOCTL_BASE, 0xa6, DrmModeGetEncoder::SIZE);
pub const DRM_IOCTL_MODE_GETCONNECTOR: u64 = iowr(DRM_IOCTL_BASE, 0xa7, DrmModeGetConnector::SIZE);
pub const DRM_IOCTL_MODE_GETPROPERTY: u64 = iowr(DRM_IOCTL_BASE, 0xaa, DrmModeGetProperty::SIZE);
pub const DRM_IOCTL_MODE_GETFB: u64 = iowr(DRM_IOCTL_BASE, 0xad, DrmModeFbCmd::SIZE);
pub const DRM_IOCTL_MODE_RMFB: u64 = iowr(DRM_IOCTL_BASE, 0xaf, 4);
pub const DRM_IOCTL_MODE_SETCRTC: u64 = iowr(DRM_IOCTL_BASE, 0xa2, DrmModeCrtc::SIZE);
pub const DRM_IOCTL_MODE_PAGE_FLIP: u64 = iowr(DRM_IOCTL_BASE, 0xb0, DrmModeCrtcPageFlip::SIZE);
pub const DRM_IOCTL_MODE_CREATE_DUMB: u64 = iowr(DRM_IOCTL_BASE, 0xb2, DrmModeCreateDumb::SIZE);
pub const DRM_IOCTL_MODE_MAP_DUMB: u64 = iowr(DRM_IOCTL_BASE, 0xb3, DrmModeMapDumb::SIZE);
pub const DRM_IOCTL_MODE_DESTROY_DUMB: u64 = iowr(DRM_IOCTL_BASE, 0xb4, DrmModeDestroyDumb::SIZE);
pub const DRM_IOCTL_MODE_GETPLANERESOURCES: u64 =
    iowr(DRM_IOCTL_BASE, 0xb5, DrmModeGetPlaneRes::SIZE);
pub const DRM_IOCTL_MODE_GETPLANE: u64 = iowr(DRM_IOCTL_BASE, 0xb6, DrmModeGetPlane::SIZE);
pub const DRM_IOCTL_MODE_ADDFB2: u64 = iowr(DRM_IOCTL_BASE, 0xb8, DrmModeFbCmd2::SIZE);
pub const DRM_IOCTL_MODE_OBJ_GETPROPERTIES: u64 =
    iowr(DRM_IOCTL_BASE, 0xb9, DrmModeObjGetProperties::SIZE);
pub const DRM_IOCTL_MODE_CLOSEFB: u64 = iowr(DRM_IOCTL_BASE, 0xd0, DrmModeCloseFb::SIZE);

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_ne_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

fn read_i32(bytes: &[u8], offset: usize) -> Option<i32> {
    Some(i32::from_ne_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

fn read_u64(bytes: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_ne_bytes(
        bytes.get(offset..offset + 8)?.try_into().ok()?,
    ))
}

fn write_u32(bytes: &mut [u8], offset: usize, value: u32) -> bool {
    let Some(target) = bytes.get_mut(offset..offset + 4) else {
        return false;
    };
    target.copy_from_slice(&value.to_ne_bytes());
    true
}

fn write_i32(bytes: &mut [u8], offset: usize, value: i32) -> bool {
    let Some(target) = bytes.get_mut(offset..offset + 4) else {
        return false;
    };
    target.copy_from_slice(&value.to_ne_bytes());
    true
}

fn write_u64(bytes: &mut [u8], offset: usize, value: u64) -> bool {
    let Some(target) = bytes.get_mut(offset..offset + 8) else {
        return false;
    };
    target.copy_from_slice(&value.to_ne_bytes());
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrmVersion {
    pub version_major: i32,
    pub version_minor: i32,
    pub version_patchlevel: i32,
    pub name_len: u64,
    pub name_ptr: u64,
    pub date_len: u64,
    pub date_ptr: u64,
    pub desc_len: u64,
    pub desc_ptr: u64,
}

impl DrmVersion {
    pub const SIZE: usize = 64;

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        Some(Self {
            version_major: read_i32(bytes, 0)?,
            version_minor: read_i32(bytes, 4)?,
            version_patchlevel: read_i32(bytes, 8)?,
            name_len: read_u64(bytes, 16)?,
            name_ptr: read_u64(bytes, 24)?,
            date_len: read_u64(bytes, 32)?,
            date_ptr: read_u64(bytes, 40)?,
            desc_len: read_u64(bytes, 48)?,
            desc_ptr: read_u64(bytes, 56)?,
        })
    }

    pub fn write_to_bytes(self, bytes: &mut [u8]) -> bool {
        write_i32(bytes, 0, self.version_major)
            && write_i32(bytes, 4, self.version_minor)
            && write_i32(bytes, 8, self.version_patchlevel)
            && write_u64(bytes, 16, self.name_len)
            && write_u64(bytes, 24, self.name_ptr)
            && write_u64(bytes, 32, self.date_len)
            && write_u64(bytes, 40, self.date_ptr)
            && write_u64(bytes, 48, self.desc_len)
            && write_u64(bytes, 56, self.desc_ptr)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrmGetCap {
    pub capability: u64,
    pub value: u64,
}

impl DrmGetCap {
    pub const SIZE: usize = 16;

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        Some(Self {
            capability: read_u64(bytes, 0)?,
            value: read_u64(bytes, 8)?,
        })
    }

    pub fn write_to_bytes(self, bytes: &mut [u8]) -> bool {
        write_u64(bytes, 0, self.capability) && write_u64(bytes, 8, self.value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrmSetClientCap {
    pub capability: u64,
    pub value: u64,
}

impl DrmSetClientCap {
    pub const SIZE: usize = 16;

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        Some(Self {
            capability: read_u64(bytes, 0)?,
            value: read_u64(bytes, 8)?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrmModeCardRes {
    pub fb_id_ptr: u64,
    pub crtc_id_ptr: u64,
    pub connector_id_ptr: u64,
    pub encoder_id_ptr: u64,
    pub count_fbs: u32,
    pub count_crtcs: u32,
    pub count_connectors: u32,
    pub count_encoders: u32,
    pub min_width: u32,
    pub max_width: u32,
    pub min_height: u32,
    pub max_height: u32,
}

impl DrmModeCardRes {
    pub const SIZE: usize = 64;

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        Some(Self {
            fb_id_ptr: read_u64(bytes, 0)?,
            crtc_id_ptr: read_u64(bytes, 8)?,
            connector_id_ptr: read_u64(bytes, 16)?,
            encoder_id_ptr: read_u64(bytes, 24)?,
            count_fbs: read_u32(bytes, 32)?,
            count_crtcs: read_u32(bytes, 36)?,
            count_connectors: read_u32(bytes, 40)?,
            count_encoders: read_u32(bytes, 44)?,
            min_width: read_u32(bytes, 48)?,
            max_width: read_u32(bytes, 52)?,
            min_height: read_u32(bytes, 56)?,
            max_height: read_u32(bytes, 60)?,
        })
    }

    pub fn write_to_bytes(self, bytes: &mut [u8]) -> bool {
        write_u64(bytes, 0, self.fb_id_ptr)
            && write_u64(bytes, 8, self.crtc_id_ptr)
            && write_u64(bytes, 16, self.connector_id_ptr)
            && write_u64(bytes, 24, self.encoder_id_ptr)
            && write_u32(bytes, 32, self.count_fbs)
            && write_u32(bytes, 36, self.count_crtcs)
            && write_u32(bytes, 40, self.count_connectors)
            && write_u32(bytes, 44, self.count_encoders)
            && write_u32(bytes, 48, self.min_width)
            && write_u32(bytes, 52, self.max_width)
            && write_u32(bytes, 56, self.min_height)
            && write_u32(bytes, 60, self.max_height)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrmModeCrtc {
    pub set_connectors_ptr: u64,
    pub count_connectors: u32,
    pub crtc_id: u32,
    pub fb_id: u32,
    pub x: u32,
    pub y: u32,
    pub gamma_size: u32,
    pub mode_valid: u32,
    pub mode: DrmModeInfo,
}

impl DrmModeCrtc {
    pub const SIZE: usize = 104;

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        Some(Self {
            set_connectors_ptr: read_u64(bytes, 0)?,
            count_connectors: read_u32(bytes, 8)?,
            crtc_id: read_u32(bytes, 12)?,
            fb_id: read_u32(bytes, 16)?,
            x: read_u32(bytes, 20)?,
            y: read_u32(bytes, 24)?,
            gamma_size: read_u32(bytes, 28)?,
            mode_valid: read_u32(bytes, 32)?,
            mode: DrmModeInfo::from_bytes(bytes.get(36..104)?)?,
        })
    }

    pub fn write_to_bytes(self, bytes: &mut [u8]) -> bool {
        write_u64(bytes, 0, self.set_connectors_ptr)
            && write_u32(bytes, 8, self.count_connectors)
            && write_u32(bytes, 12, self.crtc_id)
            && write_u32(bytes, 16, self.fb_id)
            && write_u32(bytes, 20, self.x)
            && write_u32(bytes, 24, self.y)
            && write_u32(bytes, 28, self.gamma_size)
            && write_u32(bytes, 32, self.mode_valid)
            && self
                .mode
                .write_to_bytes(bytes.get_mut(36..104).unwrap_or(&mut []))
    }
}

impl DrmModeInfo {
    pub const SIZE: usize = 68;

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let name = bytes.get(36..68)?.try_into().ok()?;
        Some(Self {
            clock: read_u32(bytes, 0)?,
            hdisplay: u16::from_ne_bytes(bytes.get(4..6)?.try_into().ok()?),
            hsync_start: u16::from_ne_bytes(bytes.get(6..8)?.try_into().ok()?),
            hsync_end: u16::from_ne_bytes(bytes.get(8..10)?.try_into().ok()?),
            htotal: u16::from_ne_bytes(bytes.get(10..12)?.try_into().ok()?),
            hskew: u16::from_ne_bytes(bytes.get(12..14)?.try_into().ok()?),
            vdisplay: u16::from_ne_bytes(bytes.get(14..16)?.try_into().ok()?),
            vsync_start: u16::from_ne_bytes(bytes.get(16..18)?.try_into().ok()?),
            vsync_end: u16::from_ne_bytes(bytes.get(18..20)?.try_into().ok()?),
            vtotal: u16::from_ne_bytes(bytes.get(20..22)?.try_into().ok()?),
            vscan: u16::from_ne_bytes(bytes.get(22..24)?.try_into().ok()?),
            vrefresh: read_u32(bytes, 24)?,
            flags: read_u32(bytes, 28)?,
            mode_type: read_u32(bytes, 32)?,
            name,
        })
    }

    pub fn write_to_bytes(self, bytes: &mut [u8]) -> bool {
        let Some(name) = bytes.get_mut(36..68) else {
            return false;
        };
        name.copy_from_slice(&self.name);
        write_u32(bytes, 0, self.clock)
            && bytes
                .get_mut(4..6)
                .map(|target| target.copy_from_slice(&self.hdisplay.to_ne_bytes()))
                .is_some()
            && bytes
                .get_mut(6..8)
                .map(|target| target.copy_from_slice(&self.hsync_start.to_ne_bytes()))
                .is_some()
            && bytes
                .get_mut(8..10)
                .map(|target| target.copy_from_slice(&self.hsync_end.to_ne_bytes()))
                .is_some()
            && bytes
                .get_mut(10..12)
                .map(|target| target.copy_from_slice(&self.htotal.to_ne_bytes()))
                .is_some()
            && bytes
                .get_mut(12..14)
                .map(|target| target.copy_from_slice(&self.hskew.to_ne_bytes()))
                .is_some()
            && bytes
                .get_mut(14..16)
                .map(|target| target.copy_from_slice(&self.vdisplay.to_ne_bytes()))
                .is_some()
            && bytes
                .get_mut(16..18)
                .map(|target| target.copy_from_slice(&self.vsync_start.to_ne_bytes()))
                .is_some()
            && bytes
                .get_mut(18..20)
                .map(|target| target.copy_from_slice(&self.vsync_end.to_ne_bytes()))
                .is_some()
            && bytes
                .get_mut(20..22)
                .map(|target| target.copy_from_slice(&self.vtotal.to_ne_bytes()))
                .is_some()
            && bytes
                .get_mut(22..24)
                .map(|target| target.copy_from_slice(&self.vscan.to_ne_bytes()))
                .is_some()
            && write_u32(bytes, 24, self.vrefresh)
            && write_u32(bytes, 28, self.flags)
            && write_u32(bytes, 32, self.mode_type)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrmModeGetEncoder {
    pub encoder_id: u32,
    pub encoder_type: u32,
    pub crtc_id: u32,
    pub possible_crtcs: u32,
    pub possible_clones: u32,
}

impl DrmModeGetEncoder {
    pub const SIZE: usize = 20;

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        Some(Self {
            encoder_id: read_u32(bytes, 0)?,
            encoder_type: read_u32(bytes, 4)?,
            crtc_id: read_u32(bytes, 8)?,
            possible_crtcs: read_u32(bytes, 12)?,
            possible_clones: read_u32(bytes, 16)?,
        })
    }

    pub fn write_to_bytes(self, bytes: &mut [u8]) -> bool {
        write_u32(bytes, 0, self.encoder_id)
            && write_u32(bytes, 4, self.encoder_type)
            && write_u32(bytes, 8, self.crtc_id)
            && write_u32(bytes, 12, self.possible_crtcs)
            && write_u32(bytes, 16, self.possible_clones)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrmModeGetConnector {
    pub encoders_ptr: u64,
    pub modes_ptr: u64,
    pub props_ptr: u64,
    pub prop_values_ptr: u64,
    pub count_modes: u32,
    pub count_props: u32,
    pub count_encoders: u32,
    pub encoder_id: u32,
    pub connector_id: u32,
    pub connector_type: u32,
    pub connector_type_id: u32,
    pub connection: u32,
    pub mm_width: u32,
    pub mm_height: u32,
    pub subpixel: u32,
    pub pad: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrmModeGetProperty {
    pub values_ptr: u64,
    pub enum_blob_ptr: u64,
    pub prop_id: u32,
    pub flags: u32,
    pub name: [u8; 32],
    pub count_values: u32,
    pub count_enum_blobs: u32,
}

impl DrmModeGetProperty {
    pub const SIZE: usize = 64;

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        Some(Self {
            values_ptr: read_u64(bytes, 0)?,
            enum_blob_ptr: read_u64(bytes, 8)?,
            prop_id: read_u32(bytes, 16)?,
            flags: read_u32(bytes, 20)?,
            name: bytes.get(24..56)?.try_into().ok()?,
            count_values: read_u32(bytes, 56)?,
            count_enum_blobs: read_u32(bytes, 60)?,
        })
    }

    pub fn write_to_bytes(self, bytes: &mut [u8]) -> bool {
        let Some(name) = bytes.get_mut(24..56) else {
            return false;
        };
        name.copy_from_slice(&self.name);
        write_u64(bytes, 0, self.values_ptr)
            && write_u64(bytes, 8, self.enum_blob_ptr)
            && write_u32(bytes, 16, self.prop_id)
            && write_u32(bytes, 20, self.flags)
            && write_u32(bytes, 56, self.count_values)
            && write_u32(bytes, 60, self.count_enum_blobs)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrmModeObjGetProperties {
    pub props_ptr: u64,
    pub prop_values_ptr: u64,
    pub count_props: u32,
    pub obj_id: u32,
    pub obj_type: u32,
}

impl DrmModeObjGetProperties {
    pub const SIZE: usize = 32;

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        Some(Self {
            props_ptr: read_u64(bytes, 0)?,
            prop_values_ptr: read_u64(bytes, 8)?,
            count_props: read_u32(bytes, 16)?,
            obj_id: read_u32(bytes, 20)?,
            obj_type: read_u32(bytes, 24)?,
        })
    }

    pub fn write_to_bytes(self, bytes: &mut [u8]) -> bool {
        write_u64(bytes, 0, self.props_ptr)
            && write_u64(bytes, 8, self.prop_values_ptr)
            && write_u32(bytes, 16, self.count_props)
            && write_u32(bytes, 20, self.obj_id)
            && write_u32(bytes, 24, self.obj_type)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrmModePropertyEnum {
    pub value: u64,
    pub name: [u8; 32],
}

impl DrmModePropertyEnum {
    pub const SIZE: usize = 40;
}

impl DrmModeGetConnector {
    pub const SIZE: usize = 80;

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        Some(Self {
            encoders_ptr: read_u64(bytes, 0)?,
            modes_ptr: read_u64(bytes, 8)?,
            props_ptr: read_u64(bytes, 16)?,
            prop_values_ptr: read_u64(bytes, 24)?,
            count_modes: read_u32(bytes, 32)?,
            count_props: read_u32(bytes, 36)?,
            count_encoders: read_u32(bytes, 40)?,
            encoder_id: read_u32(bytes, 44)?,
            connector_id: read_u32(bytes, 48)?,
            connector_type: read_u32(bytes, 52)?,
            connector_type_id: read_u32(bytes, 56)?,
            connection: read_u32(bytes, 60)?,
            mm_width: read_u32(bytes, 64)?,
            mm_height: read_u32(bytes, 68)?,
            subpixel: read_u32(bytes, 72)?,
            pad: read_u32(bytes, 76)?,
        })
    }

    pub fn write_to_bytes(self, bytes: &mut [u8]) -> bool {
        write_u64(bytes, 0, self.encoders_ptr)
            && write_u64(bytes, 8, self.modes_ptr)
            && write_u64(bytes, 16, self.props_ptr)
            && write_u64(bytes, 24, self.prop_values_ptr)
            && write_u32(bytes, 32, self.count_modes)
            && write_u32(bytes, 36, self.count_props)
            && write_u32(bytes, 40, self.count_encoders)
            && write_u32(bytes, 44, self.encoder_id)
            && write_u32(bytes, 48, self.connector_id)
            && write_u32(bytes, 52, self.connector_type)
            && write_u32(bytes, 56, self.connector_type_id)
            && write_u32(bytes, 60, self.connection)
            && write_u32(bytes, 64, self.mm_width)
            && write_u32(bytes, 68, self.mm_height)
            && write_u32(bytes, 72, self.subpixel)
            && write_u32(bytes, 76, self.pad)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrmModeGetPlaneRes {
    pub plane_id_ptr: u64,
    pub count_planes: u32,
}

impl DrmModeGetPlaneRes {
    pub const SIZE: usize = 16;

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        Some(Self {
            plane_id_ptr: read_u64(bytes, 0)?,
            count_planes: read_u32(bytes, 8)?,
        })
    }

    pub fn write_to_bytes(self, bytes: &mut [u8]) -> bool {
        write_u64(bytes, 0, self.plane_id_ptr) && write_u32(bytes, 8, self.count_planes)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrmModeGetPlane {
    pub plane_id: u32,
    pub crtc_id: u32,
    pub fb_id: u32,
    pub possible_crtcs: u32,
    pub gamma_size: u32,
    pub count_format_types: u32,
    pub format_type_ptr: u64,
}

impl DrmModeGetPlane {
    pub const SIZE: usize = 32;

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        Some(Self {
            plane_id: read_u32(bytes, 0)?,
            crtc_id: read_u32(bytes, 4)?,
            fb_id: read_u32(bytes, 8)?,
            possible_crtcs: read_u32(bytes, 12)?,
            gamma_size: read_u32(bytes, 16)?,
            count_format_types: read_u32(bytes, 20)?,
            format_type_ptr: read_u64(bytes, 24)?,
        })
    }

    pub fn write_to_bytes(self, bytes: &mut [u8]) -> bool {
        write_u32(bytes, 0, self.plane_id)
            && write_u32(bytes, 4, self.crtc_id)
            && write_u32(bytes, 8, self.fb_id)
            && write_u32(bytes, 12, self.possible_crtcs)
            && write_u32(bytes, 16, self.gamma_size)
            && write_u32(bytes, 20, self.count_format_types)
            && write_u64(bytes, 24, self.format_type_ptr)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrmModeFbCmd {
    pub fb_id: u32,
    pub width: u32,
    pub height: u32,
    pub pitch: u32,
    pub bpp: u32,
    pub depth: u32,
    pub handle: u32,
}

impl DrmModeFbCmd {
    pub const SIZE: usize = 28;

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        Some(Self {
            fb_id: read_u32(bytes, 0)?,
            width: read_u32(bytes, 4)?,
            height: read_u32(bytes, 8)?,
            pitch: read_u32(bytes, 12)?,
            bpp: read_u32(bytes, 16)?,
            depth: read_u32(bytes, 20)?,
            handle: read_u32(bytes, 24)?,
        })
    }

    pub fn write_to_bytes(self, bytes: &mut [u8]) -> bool {
        write_u32(bytes, 0, self.fb_id)
            && write_u32(bytes, 4, self.width)
            && write_u32(bytes, 8, self.height)
            && write_u32(bytes, 12, self.pitch)
            && write_u32(bytes, 16, self.bpp)
            && write_u32(bytes, 20, self.depth)
            && write_u32(bytes, 24, self.handle)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrmModeFbCmd2 {
    pub fb_id: u32,
    pub width: u32,
    pub height: u32,
    pub pixel_format: u32,
    pub flags: u32,
    pub handles: [u32; 4],
    pub pitches: [u32; 4],
    pub offsets: [u32; 4],
    pub modifiers: [u64; 4],
}

impl DrmModeFbCmd2 {
    pub const SIZE: usize = 104;

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let mut handles = [0u32; 4];
        let mut pitches = [0u32; 4];
        let mut offsets = [0u32; 4];
        let mut modifiers = [0u64; 4];
        for (index, slot) in handles.iter_mut().enumerate() {
            *slot = read_u32(bytes, 20 + index * 4)?;
        }
        for (index, slot) in pitches.iter_mut().enumerate() {
            *slot = read_u32(bytes, 36 + index * 4)?;
        }
        for (index, slot) in offsets.iter_mut().enumerate() {
            *slot = read_u32(bytes, 52 + index * 4)?;
        }
        for (index, slot) in modifiers.iter_mut().enumerate() {
            *slot = read_u64(bytes, 72 + index * 8)?;
        }
        Some(Self {
            fb_id: read_u32(bytes, 0)?,
            width: read_u32(bytes, 4)?,
            height: read_u32(bytes, 8)?,
            pixel_format: read_u32(bytes, 12)?,
            flags: read_u32(bytes, 16)?,
            handles,
            pitches,
            offsets,
            modifiers,
        })
    }

    pub fn write_to_bytes(self, bytes: &mut [u8]) -> bool {
        let mut ok = write_u32(bytes, 0, self.fb_id)
            && write_u32(bytes, 4, self.width)
            && write_u32(bytes, 8, self.height)
            && write_u32(bytes, 12, self.pixel_format)
            && write_u32(bytes, 16, self.flags);
        for (index, value) in self.handles.into_iter().enumerate() {
            ok &= write_u32(bytes, 20 + index * 4, value);
        }
        for (index, value) in self.pitches.into_iter().enumerate() {
            ok &= write_u32(bytes, 36 + index * 4, value);
        }
        for (index, value) in self.offsets.into_iter().enumerate() {
            ok &= write_u32(bytes, 52 + index * 4, value);
        }
        for (index, value) in self.modifiers.into_iter().enumerate() {
            ok &= write_u64(bytes, 72 + index * 8, value);
        }
        ok
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrmModeCreateDumb {
    pub height: u32,
    pub width: u32,
    pub bpp: u32,
    pub flags: u32,
    pub handle: u32,
    pub pitch: u32,
    pub size: u64,
}

impl DrmModeCreateDumb {
    pub const SIZE: usize = 32;

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        Some(Self {
            height: read_u32(bytes, 0)?,
            width: read_u32(bytes, 4)?,
            bpp: read_u32(bytes, 8)?,
            flags: read_u32(bytes, 12)?,
            handle: read_u32(bytes, 16)?,
            pitch: read_u32(bytes, 20)?,
            size: read_u64(bytes, 24)?,
        })
    }

    pub fn write_to_bytes(self, bytes: &mut [u8]) -> bool {
        write_u32(bytes, 0, self.height)
            && write_u32(bytes, 4, self.width)
            && write_u32(bytes, 8, self.bpp)
            && write_u32(bytes, 12, self.flags)
            && write_u32(bytes, 16, self.handle)
            && write_u32(bytes, 20, self.pitch)
            && write_u64(bytes, 24, self.size)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrmModeMapDumb {
    pub handle: u32,
    pub pad: u32,
    pub offset: u64,
}

impl DrmModeMapDumb {
    pub const SIZE: usize = 16;

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        Some(Self {
            handle: read_u32(bytes, 0)?,
            pad: read_u32(bytes, 4)?,
            offset: read_u64(bytes, 8)?,
        })
    }

    pub fn write_to_bytes(self, bytes: &mut [u8]) -> bool {
        write_u32(bytes, 0, self.handle)
            && write_u32(bytes, 4, self.pad)
            && write_u64(bytes, 8, self.offset)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrmModeDestroyDumb {
    pub handle: u32,
}

impl DrmModeDestroyDumb {
    pub const SIZE: usize = 4;

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        Some(Self {
            handle: read_u32(bytes, 0)?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrmModeCloseFb {
    pub fb_id: u32,
    pub pad: u32,
}

impl DrmModeCloseFb {
    pub const SIZE: usize = 8;

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        Some(Self {
            fb_id: read_u32(bytes, 0)?,
            pad: read_u32(bytes, 4)?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrmModeCrtcPageFlip {
    pub crtc_id: u32,
    pub fb_id: u32,
    pub flags: u32,
    pub reserved: u32,
    pub user_data: u64,
}

impl DrmModeCrtcPageFlip {
    pub const SIZE: usize = 24;

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        Some(Self {
            crtc_id: read_u32(bytes, 0)?,
            fb_id: read_u32(bytes, 4)?,
            flags: read_u32(bytes, 8)?,
            reserved: read_u32(bytes, 12)?,
            user_data: read_u64(bytes, 16)?,
        })
    }
}

pub fn encode_u32_array(values: &[u32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(values.len() * 4);
    for value in values {
        bytes.extend_from_slice(&value.to_ne_bytes());
    }
    bytes
}

pub fn encode_u64_array(values: &[u64]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(values.len() * 8);
    for value in values {
        bytes.extend_from_slice(&value.to_ne_bytes());
    }
    bytes
}

pub fn encode_modes(modes: &[DrmModeInfo]) -> Vec<u8> {
    let mut bytes = vec![0u8; modes.len() * DrmModeInfo::SIZE];
    for (index, mode) in modes.iter().copied().enumerate() {
        let start = index * DrmModeInfo::SIZE;
        let end = start + DrmModeInfo::SIZE;
        let _ = mode.write_to_bytes(&mut bytes[start..end]);
    }
    bytes
}

pub fn encode_property_enums(values: &[DrmPropertyEnumValue]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(values.len() * DrmModePropertyEnum::SIZE);
    for value in values {
        bytes.extend_from_slice(&value.value.to_ne_bytes());
        let mut name = [0u8; 32];
        let raw = value.name.as_bytes();
        let len = raw.len().min(name.len().saturating_sub(1));
        name[..len].copy_from_slice(&raw[..len]);
        bytes.extend_from_slice(&name);
    }
    bytes
}
