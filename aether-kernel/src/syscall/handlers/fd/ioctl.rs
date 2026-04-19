use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::fs::{DevPtsSlaveFile, PtmxMasterFile};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::KernelSyscallContext;
use crate::syscall::SyscallDisposition;
use aether_drivers::drm::{DrmDevice, DrmModeInfo, ioctl as drm_abi};
use aether_drivers::{DrmFile, EvdevFile};
use aether_terminal::{ConsoleCore, LinuxTermios, LinuxTermios2, LinuxVtMode, LinuxWinSize};
use aether_vfs::{FsError, IoctlResponse};

const DRM_USER_BLOB_MAX_SIZE: usize = 64 * 1024;
const TCGETS: u64 = 0x5401;
const TCSETS: u64 = 0x5402;
const TCSETSW: u64 = 0x5403;
const TCSETSF: u64 = 0x5404;
const TIOCGPGRP: u64 = 0x540f;
const TIOCSPGRP: u64 = 0x5410;
const TIOCSWINSZ: u64 = 0x5414;
const TIOCSCTTY: u64 = 0x540e;
const TIOCNOTTY: u64 = 0x5422;
const TIOCGPTN: u64 = 0x8004_5430;
const TIOCSPTLCK: u64 = 0x4004_5431;
const TIOCGPTLCK: u64 = 0x8004_5439;
const TCFLSH: u64 = 0x540b;
const TIOCNXCL: u64 = 0x540d;
const KDSETMODE: u64 = 0x4b3a;
const KDSKBMODE: u64 = 0x4b45;
const VT_SETMODE: u64 = 0x5602;
const VT_ACTIVATE: u64 = 0x5606;
const VT_WAITACTIVE: u64 = 0x5607;
const TCGETS2: u64 = 0x802c_542a;
const TCSETS2: u64 = 0x402c_542b;

crate::declare_syscall!(
    pub struct IoctlSyscall => nr::IOCTL, "ioctl", |ctx, args| {
        SyscallDisposition::Return(ctx.ioctl_fd(args.get(0), args.get(1), args.get(2)))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_ioctl_fd(
        &mut self,
        fd: u64,
        command: u64,
        argument: u64,
    ) -> SysResult<u64> {
        let descriptor = self.process.files.get(fd as u32).ok_or(SysErr::BadFd)?;
        let file_ref = descriptor.file.clone();
        let file = file_ref.lock();
        if let Some(ptmx) = file
            .file_ops()
            .and_then(|ops| ops.as_any().downcast_ref::<PtmxMasterFile>())
            && let Some(result) = self.syscall_ptmx_ioctl(ptmx, command, argument)?
        {
            drop(file);
            return self.finish_ioctl_result(argument, result);
        }
        if let Some(console) = file
            .file_ops()
            .and_then(|ops| ops.as_any().downcast_ref::<ConsoleCore>())
            && let Some(result) = self.syscall_tty_ioctl(console, command, argument)?
        {
            drop(file);
            return self.finish_ioctl_result(argument, result);
        }
        if let Some(slave) = file
            .file_ops()
            .and_then(|ops| ops.as_any().downcast_ref::<DevPtsSlaveFile>())
            && let Some(result) = self.syscall_tty_ioctl(slave.tty(), command, argument)?
        {
            drop(file);
            return self.finish_ioctl_result(argument, result);
        }
        if let Some(device) = file.file_ops_arc().and_then(|ops| {
            ops.as_any()
                .downcast_ref::<DrmFile>()
                .map(|drm| drm.device().clone())
        }) {
            drop(file);
            return self.syscall_drm_ioctl(device.as_ref(), command, argument);
        }
        if let Some(evdev) = file
            .file_ops()
            .and_then(|ops| ops.as_any().downcast_ref::<EvdevFile>())
            && let Some(result) = self.syscall_evdev_ioctl(evdev, command, argument)?
        {
            drop(file);
            return self.finish_ioctl_result(argument, result);
        }
        let result = match file.ioctl(command, argument) {
            Ok(result) => result,
            Err(FsError::Unsupported) => return Err(SysErr::NoTty),
            Err(error) => return Err(SysErr::from(error)),
        };
        drop(file);
        self.finish_ioctl_result(argument, result)
    }

    fn finish_ioctl_result(&mut self, argument: u64, result: IoctlResponse) -> SysResult<u64> {
        match result {
            IoctlResponse::None(value) => Ok(value),
            IoctlResponse::Data(bytes) => {
                self.write_user_buffer(argument, &bytes)?;
                Ok(0)
            }
            IoctlResponse::DataValue(bytes, value) => {
                self.write_user_buffer(argument, &bytes)?;
                Ok(value)
            }
        }
    }

    fn syscall_tty_ioctl(
        &mut self,
        console: &ConsoleCore,
        command: u64,
        argument: u64,
    ) -> SysResult<Option<IoctlResponse>> {
        let result = match command {
            TCGETS => Some(IoctlResponse::Data(console.termios().to_bytes().to_vec())),
            TCGETS2 => Some(IoctlResponse::Data(console.termios2().to_bytes().to_vec())),
            TCSETS | TCSETSW | TCSETSF => {
                let termios = LinuxTermios::from_bytes(&self.syscall_read_user_exact_buffer(
                    argument,
                    core::mem::size_of::<LinuxTermios>(),
                )?)
                .ok_or(SysErr::Fault)?;
                console.set_termios(termios);
                Some(IoctlResponse::success())
            }
            TCSETS2 => {
                let termios = LinuxTermios2::from_bytes(&self.syscall_read_user_exact_buffer(
                    argument,
                    core::mem::size_of::<LinuxTermios2>(),
                )?)
                .ok_or(SysErr::Fault)?;
                console.set_termios2(termios);
                Some(IoctlResponse::success())
            }
            TIOCGPGRP => Some(IoctlResponse::Data(
                console.process_group().to_ne_bytes().to_vec(),
            )),
            TIOCSPGRP => {
                let bytes =
                    self.syscall_read_user_exact_buffer(argument, core::mem::size_of::<i32>())?;
                let process_group =
                    i32::from_ne_bytes(bytes.as_slice().try_into().map_err(|_| SysErr::Fault)?);
                console.set_process_group(process_group);
                Some(IoctlResponse::success())
            }
            TIOCSWINSZ => {
                let winsize = LinuxWinSize::from_bytes(&self.syscall_read_user_exact_buffer(
                    argument,
                    core::mem::size_of::<LinuxWinSize>(),
                )?)
                .ok_or(SysErr::Fault)?;
                console.set_winsize(winsize);
                Some(IoctlResponse::success())
            }
            TIOCSCTTY => {
                console.set_process_group(self.process.identity.process_group as i32);
                Some(IoctlResponse::success())
            }
            TIOCNOTTY => Some(IoctlResponse::success()),
            TCFLSH => Some(IoctlResponse::success()),
            TIOCNXCL => Some(IoctlResponse::success()),
            KDSETMODE => {
                console.set_tty_mode(argument as i32);
                Some(IoctlResponse::success())
            }
            KDSKBMODE => {
                console.set_keyboard_mode(argument as i32);
                Some(IoctlResponse::success())
            }
            VT_SETMODE => {
                let mode = LinuxVtMode::from_bytes(&self.syscall_read_user_exact_buffer(
                    argument,
                    core::mem::size_of::<LinuxVtMode>(),
                )?)
                .ok_or(SysErr::Fault)?;
                console.set_vt_mode(mode);
                Some(IoctlResponse::success())
            }
            VT_ACTIVATE | VT_WAITACTIVE => {
                if argument != 0 {
                    console.set_active_vt(argument as u16);
                }
                Some(IoctlResponse::success())
            }
            _ => None,
        };
        Ok(result)
    }

    fn syscall_ptmx_ioctl(
        &mut self,
        master: &PtmxMasterFile,
        command: u64,
        argument: u64,
    ) -> SysResult<Option<IoctlResponse>> {
        let result = match command {
            TIOCGPTN => Some(IoctlResponse::Data(
                master.pty_number().to_ne_bytes().to_vec(),
            )),
            TIOCSPTLCK => {
                let bytes =
                    self.syscall_read_user_exact_buffer(argument, core::mem::size_of::<i32>())?;
                let locked =
                    i32::from_ne_bytes(bytes.as_slice().try_into().map_err(|_| SysErr::Fault)?)
                        != 0;
                master.set_locked(locked);
                Some(IoctlResponse::success())
            }
            TIOCGPTLCK => Some(IoctlResponse::Data(
                (if master.locked() { 1i32 } else { 0i32 })
                    .to_ne_bytes()
                    .to_vec(),
            )),
            _ => self.syscall_tty_ioctl(master.slave(), command, argument)?,
        };
        Ok(result)
    }

    fn syscall_evdev_ioctl(
        &mut self,
        evdev: &EvdevFile,
        command: u64,
        argument: u64,
    ) -> SysResult<Option<IoctlResponse>> {
        const EVIOCSCLOCKID: u64 = 0x400445a0;

        let result = match command {
            EVIOCSCLOCKID => {
                let bytes =
                    self.syscall_read_user_exact_buffer(argument, core::mem::size_of::<i32>())?;
                let clock_id =
                    i32::from_ne_bytes(bytes.as_slice().try_into().map_err(|_| SysErr::Fault)?);
                evdev.set_clock_id(clock_id).map_err(SysErr::from)?;
                Some(IoctlResponse::success())
            }
            _ => None,
        };
        Ok(result)
    }

    fn syscall_drm_ioctl(
        &mut self,
        device: &DrmDevice,
        command: u64,
        argument: u64,
    ) -> SysResult<u64> {
        let dir = drm_abi::ioctl_dir(command);
        let size = drm_abi::ioctl_size(command);
        let mut bytes = if size == 0 {
            alloc::vec::Vec::new()
        } else if argument == 0 {
            return Err(SysErr::Fault);
        } else if (dir & drm_abi::IOC_WRITE) != 0 {
            self.syscall_read_user_exact_buffer(argument, size)?
        } else {
            alloc::vec![0u8; size]
        };

        match command {
            drm_abi::DRM_IOCTL_VERSION => {
                let mut version = drm_abi::DrmVersion::from_bytes(&bytes).ok_or(SysErr::Fault)?;
                let info = device.driver_info();
                let mut user = DrmUserWriter::new(self);
                user.write_bytes(
                    version.name_ptr,
                    version.name_len as usize,
                    info.name.as_bytes(),
                )?;
                user.write_bytes(
                    version.date_ptr,
                    version.date_len as usize,
                    info.date.as_bytes(),
                )?;
                user.write_bytes(
                    version.desc_ptr,
                    version.desc_len as usize,
                    info.description.as_bytes(),
                )?;
                version.version_major = 1;
                version.version_minor = 0;
                version.version_patchlevel = 0;
                version.name_len = info.name.len() as u64;
                version.date_len = info.date.len() as u64;
                version.desc_len = info.description.len() as u64;
                if !version.write_to_bytes(&mut bytes) {
                    return Err(SysErr::Fault);
                }
            }
            drm_abi::DRM_IOCTL_GET_CAP => {
                let mut request = drm_abi::DrmGetCap::from_bytes(&bytes).ok_or(SysErr::Fault)?;
                request.value = device.get_cap(request.capability);
                if !request.write_to_bytes(&mut bytes) {
                    return Err(SysErr::Fault);
                }
            }
            drm_abi::DRM_IOCTL_SET_CLIENT_CAP => {
                let request = drm_abi::DrmSetClientCap::from_bytes(&bytes).ok_or(SysErr::Fault)?;
                device
                    .set_client_cap(request.capability, request.value)
                    .map_err(SysErr::from)?;
            }
            drm_abi::DRM_IOCTL_SET_MASTER => {
                device
                    .set_master(self.process.identity.pid)
                    .map_err(SysErr::from)?;
            }
            drm_abi::DRM_IOCTL_DROP_MASTER => {
                device
                    .drop_master(self.process.identity.pid)
                    .map_err(SysErr::from)?;
            }
            drm_abi::DRM_IOCTL_MODE_GETRESOURCES => {
                let mut request =
                    drm_abi::DrmModeCardRes::from_bytes(&bytes).ok_or(SysErr::Fault)?;
                let snapshot = device.resources();
                let mut user = DrmUserWriter::new(self);
                user.write_u32s(
                    request.fb_id_ptr,
                    request.count_fbs as usize,
                    snapshot.framebuffer_ids.as_slice(),
                )?;
                user.write_u32s(
                    request.crtc_id_ptr,
                    request.count_crtcs as usize,
                    snapshot.crtc_ids.as_slice(),
                )?;
                user.write_u32s(
                    request.connector_id_ptr,
                    request.count_connectors as usize,
                    snapshot.connector_ids.as_slice(),
                )?;
                user.write_u32s(
                    request.encoder_id_ptr,
                    request.count_encoders as usize,
                    snapshot.encoder_ids.as_slice(),
                )?;
                request.count_fbs = snapshot.framebuffer_ids.len() as u32;
                request.count_crtcs = snapshot.crtc_ids.len() as u32;
                request.count_connectors = snapshot.connector_ids.len() as u32;
                request.count_encoders = snapshot.encoder_ids.len() as u32;
                request.min_width = snapshot.min_width;
                request.max_width = snapshot.max_width;
                request.min_height = snapshot.min_height;
                request.max_height = snapshot.max_height;
                if !request.write_to_bytes(&mut bytes) {
                    return Err(SysErr::Fault);
                }
            }
            drm_abi::DRM_IOCTL_MODE_GETCRTC => {
                let mut request = drm_abi::DrmModeCrtc::from_bytes(&bytes).ok_or(SysErr::Fault)?;
                let snapshot = device.get_crtc(request.crtc_id).ok_or(SysErr::NoEnt)?;
                request.fb_id = snapshot.framebuffer_id;
                request.x = snapshot.x;
                request.y = snapshot.y;
                request.gamma_size = snapshot.gamma_size;
                request.mode_valid = u32::from(snapshot.mode_valid);
                request.mode = snapshot.mode;
                if !request.write_to_bytes(&mut bytes) {
                    return Err(SysErr::Fault);
                }
            }
            drm_abi::DRM_IOCTL_MODE_GETENCODER => {
                let mut request =
                    drm_abi::DrmModeGetEncoder::from_bytes(&bytes).ok_or(SysErr::Fault)?;
                let snapshot = device
                    .get_encoder(request.encoder_id)
                    .ok_or(SysErr::NoEnt)?;
                request.encoder_type = snapshot.encoder_type;
                request.crtc_id = snapshot.crtc_id;
                request.possible_crtcs = snapshot.possible_crtcs;
                request.possible_clones = snapshot.possible_clones;
                if !request.write_to_bytes(&mut bytes) {
                    return Err(SysErr::Fault);
                }
            }
            drm_abi::DRM_IOCTL_MODE_GETCONNECTOR => {
                let mut request =
                    drm_abi::DrmModeGetConnector::from_bytes(&bytes).ok_or(SysErr::Fault)?;
                let snapshot = device
                    .get_connector(request.connector_id)
                    .ok_or(SysErr::NoEnt)?;
                let properties = device
                    .get_object_properties(
                        request.connector_id,
                        aether_drivers::drm::DRM_MODE_OBJECT_CONNECTOR,
                    )
                    .map_err(SysErr::from)?;
                let mut user = DrmUserWriter::new(self);
                user.write_modes(
                    request.modes_ptr,
                    request.count_modes as usize,
                    snapshot.modes.as_slice(),
                )?;
                user.write_u32s(
                    request.props_ptr,
                    request.count_props as usize,
                    properties.ids.as_slice(),
                )?;
                user.write_u64s(
                    request.prop_values_ptr,
                    request.count_props as usize,
                    properties.values.as_slice(),
                )?;
                user.write_u32s(
                    request.encoders_ptr,
                    request.count_encoders as usize,
                    snapshot.encoders.as_slice(),
                )?;
                request.count_modes = snapshot.modes.len() as u32;
                request.count_props = properties.ids.len() as u32;
                request.count_encoders = snapshot.encoders.len() as u32;
                request.encoder_id = snapshot.encoder_id;
                request.connector_type = snapshot.connector_type;
                request.connector_type_id = snapshot.connector_type_id;
                request.connection = snapshot.connection;
                request.mm_width = snapshot.mm_width;
                request.mm_height = snapshot.mm_height;
                request.subpixel = snapshot.subpixel;
                if !request.write_to_bytes(&mut bytes) {
                    return Err(SysErr::Fault);
                }
            }
            drm_abi::DRM_IOCTL_MODE_GETPROPERTY => {
                let mut request =
                    drm_abi::DrmModeGetProperty::from_bytes(&bytes).ok_or(SysErr::Fault)?;
                let property = device.get_property(request.prop_id).map_err(SysErr::from)?;
                let mut user = DrmUserWriter::new(self);
                user.write_u64s(
                    request.values_ptr,
                    request.count_values as usize,
                    property.values.as_slice(),
                )?;
                user.write_property_enums(
                    request.enum_blob_ptr,
                    request.count_enum_blobs as usize,
                    property.enums.as_slice(),
                )?;
                request.flags = property.flags;
                request.name = [0u8; 32];
                let name = property.name.as_bytes();
                let name_len = name.len().min(request.name.len().saturating_sub(1));
                request.name[..name_len].copy_from_slice(&name[..name_len]);
                request.count_values = property.values.len() as u32;
                request.count_enum_blobs = property.enums.len() as u32;
                if !request.write_to_bytes(&mut bytes) {
                    return Err(SysErr::Fault);
                }
            }
            drm_abi::DRM_IOCTL_MODE_SETPROPERTY => {
                let _request = drm_abi::DrmModeConnectorSetProperty::from_bytes(&bytes)
                    .ok_or(SysErr::Fault)?;
                // Match naos's permissive legacy connector property path:
                // accept the ioctl even when DPMS is not wired into backend state.
            }
            drm_abi::DRM_IOCTL_MODE_GETPROPBLOB => {
                let mut request =
                    drm_abi::DrmModeGetBlob::from_bytes(&bytes).ok_or(SysErr::Fault)?;
                let blob = device
                    .get_property_blob(request.blob_id)
                    .ok_or(SysErr::NoEnt)?;
                DrmUserWriter::new(self).write_bytes(
                    request.data,
                    request.length as usize,
                    blob.as_slice(),
                )?;
                request.length = blob.len() as u32;
                if !request.write_to_bytes(&mut bytes) {
                    return Err(SysErr::Fault);
                }
            }
            drm_abi::DRM_IOCTL_MODE_GETPLANERESOURCES => {
                let mut request =
                    drm_abi::DrmModeGetPlaneRes::from_bytes(&bytes).ok_or(SysErr::Fault)?;
                let planes = device.plane_ids();
                DrmUserWriter::new(self).write_u32s(
                    request.plane_id_ptr,
                    request.count_planes as usize,
                    planes.as_slice(),
                )?;
                request.count_planes = planes.len() as u32;
                if !request.write_to_bytes(&mut bytes) {
                    return Err(SysErr::Fault);
                }
            }
            drm_abi::DRM_IOCTL_MODE_GETPLANE => {
                let mut request =
                    drm_abi::DrmModeGetPlane::from_bytes(&bytes).ok_or(SysErr::Fault)?;
                let snapshot = device.get_plane(request.plane_id).ok_or(SysErr::NoEnt)?;
                DrmUserWriter::new(self).write_u32s(
                    request.format_type_ptr,
                    request.count_format_types as usize,
                    snapshot.format_types.as_slice(),
                )?;
                request.crtc_id = snapshot.crtc_id;
                request.fb_id = snapshot.framebuffer_id;
                request.possible_crtcs = snapshot.possible_crtcs;
                request.gamma_size = snapshot.gamma_size;
                request.count_format_types = snapshot.format_types.len() as u32;
                if !request.write_to_bytes(&mut bytes) {
                    return Err(SysErr::Fault);
                }
            }
            drm_abi::DRM_IOCTL_MODE_OBJ_GETPROPERTIES => {
                let mut request =
                    drm_abi::DrmModeObjGetProperties::from_bytes(&bytes).ok_or(SysErr::Fault)?;
                let properties = device
                    .get_object_properties(request.obj_id, request.obj_type)
                    .map_err(SysErr::from)?;
                let mut user = DrmUserWriter::new(self);
                user.write_u32s(
                    request.props_ptr,
                    request.count_props as usize,
                    properties.ids.as_slice(),
                )?;
                user.write_u64s(
                    request.prop_values_ptr,
                    request.count_props as usize,
                    properties.values.as_slice(),
                )?;
                request.count_props = properties.ids.len() as u32;
                if !request.write_to_bytes(&mut bytes) {
                    return Err(SysErr::Fault);
                }
            }
            drm_abi::DRM_IOCTL_MODE_OBJ_SETPROPERTY => {
                let _request =
                    drm_abi::DrmModeObjSetProperty::from_bytes(&bytes).ok_or(SysErr::Fault)?;
                // Keep legacy/compat property sets permissive like naos.
            }
            drm_abi::DRM_IOCTL_MODE_ATOMIC => {
                let request = drm_abi::DrmModeAtomic::from_bytes(&bytes).ok_or(SysErr::Fault)?;
                if request.reserved != 0 {
                    return Err(SysErr::Inval);
                }
                let obj_ids =
                    self.read_drm_u32_array(request.objs_ptr, request.count_objs as usize)?;
                let obj_prop_counts =
                    self.read_drm_u32_array(request.count_props_ptr, request.count_objs as usize)?;
                let prop_count = obj_prop_counts
                    .iter()
                    .try_fold(0usize, |sum, count| sum.checked_add(*count as usize))
                    .ok_or(SysErr::Inval)?;
                let prop_ids = self.read_drm_u32_array(request.props_ptr, prop_count)?;
                let prop_values = self.read_drm_u64_array(request.prop_values_ptr, prop_count)?;
                device
                    .atomic_commit(
                        request.flags,
                        obj_ids.as_slice(),
                        obj_prop_counts.as_slice(),
                        prop_ids.as_slice(),
                        prop_values.as_slice(),
                        request.user_data,
                    )
                    .map_err(SysErr::from)?;
            }
            drm_abi::DRM_IOCTL_MODE_GETFB => {
                let mut request = drm_abi::DrmModeFbCmd::from_bytes(&bytes).ok_or(SysErr::Fault)?;
                let snapshot = device.get_framebuffer(request.fb_id).ok_or(SysErr::NoEnt)?;
                request.width = snapshot.width;
                request.height = snapshot.height;
                request.pitch = snapshot.pitch;
                request.depth = snapshot.depth;
                request.bpp = snapshot.bpp;
                request.handle = snapshot.handle;
                if !request.write_to_bytes(&mut bytes) {
                    return Err(SysErr::Fault);
                }
            }
            drm_abi::DRM_IOCTL_MODE_CREATE_DUMB => {
                let mut request =
                    drm_abi::DrmModeCreateDumb::from_bytes(&bytes).ok_or(SysErr::Fault)?;
                let (handle, pitch, size) = device
                    .create_dumb(request.width, request.height, request.bpp, request.flags)
                    .map_err(SysErr::from)?;
                request.handle = handle;
                request.pitch = pitch;
                request.size = size;
                if !request.write_to_bytes(&mut bytes) {
                    return Err(SysErr::Fault);
                }
            }
            drm_abi::DRM_IOCTL_MODE_MAP_DUMB => {
                let mut request =
                    drm_abi::DrmModeMapDumb::from_bytes(&bytes).ok_or(SysErr::Fault)?;
                if request.pad != 0 {
                    return Err(SysErr::Inval);
                }
                request.offset = device.map_dumb(request.handle).map_err(SysErr::from)?;
                if !request.write_to_bytes(&mut bytes) {
                    return Err(SysErr::Fault);
                }
            }
            drm_abi::DRM_IOCTL_MODE_DESTROY_DUMB => {
                let request =
                    drm_abi::DrmModeDestroyDumb::from_bytes(&bytes).ok_or(SysErr::Fault)?;
                device.destroy_dumb(request.handle).map_err(SysErr::from)?;
            }
            drm_abi::DRM_IOCTL_MODE_ADDFB2 => {
                let mut request =
                    drm_abi::DrmModeFbCmd2::from_bytes(&bytes).ok_or(SysErr::Fault)?;
                if request.handles[1..].iter().any(|handle| *handle != 0)
                    || request.offsets.iter().any(|offset| *offset != 0)
                    || request.modifiers.iter().any(|modifier| *modifier != 0)
                {
                    return Err(SysErr::NotSup);
                }
                let fb_id = device
                    .add_framebuffer2(aether_drivers::drm::DrmFramebufferCreate {
                        width: request.width,
                        height: request.height,
                        pixel_format: request.pixel_format,
                        flags: request.flags,
                        handle: request.handles[0],
                        pitch: request.pitches[0],
                    })
                    .map_err(SysErr::from)?;
                request.fb_id = fb_id;
                if !request.write_to_bytes(&mut bytes) {
                    return Err(SysErr::Fault);
                }
            }
            drm_abi::DRM_IOCTL_MODE_CREATEPROPBLOB => {
                let mut request =
                    drm_abi::DrmModeCreateBlob::from_bytes(&bytes).ok_or(SysErr::Fault)?;
                let length = request.length as usize;
                if request.data == 0 || length == 0 || length > DRM_USER_BLOB_MAX_SIZE {
                    return Err(SysErr::Inval);
                }
                let blob = self.syscall_read_user_exact_buffer(request.data, length)?;
                request.blob_id = device
                    .create_property_blob(blob.as_slice())
                    .map_err(SysErr::from)?;
                if !request.write_to_bytes(&mut bytes) {
                    return Err(SysErr::Fault);
                }
            }
            drm_abi::DRM_IOCTL_MODE_DESTROYPROPBLOB => {
                let request =
                    drm_abi::DrmModeDestroyBlob::from_bytes(&bytes).ok_or(SysErr::Fault)?;
                if request.blob_id == 0 {
                    return Err(SysErr::Inval);
                }
                device
                    .destroy_property_blob(request.blob_id)
                    .map_err(SysErr::from)?;
            }
            drm_abi::DRM_IOCTL_MODE_CLOSEFB => {
                let request = drm_abi::DrmModeCloseFb::from_bytes(&bytes).ok_or(SysErr::Fault)?;
                if request.pad != 0 {
                    return Err(SysErr::Inval);
                }
                device
                    .remove_framebuffer(request.fb_id)
                    .map_err(SysErr::from)?;
            }
            drm_abi::DRM_IOCTL_MODE_RMFB => {
                let fb_id = u32::from_ne_bytes(bytes[..4].try_into().map_err(|_| SysErr::Fault)?);
                device.remove_framebuffer(fb_id).map_err(SysErr::from)?;
            }
            drm_abi::DRM_IOCTL_MODE_SETCRTC => {
                let request = drm_abi::DrmModeCrtc::from_bytes(&bytes).ok_or(SysErr::Fault)?;
                if request.crtc_id
                    != device
                        .get_crtc(request.crtc_id)
                        .ok_or(SysErr::NoEnt)?
                        .crtc_id
                {
                    return Err(SysErr::NoEnt);
                }
                if request.count_connectors != 0 && request.set_connectors_ptr != 0 {
                    let connector_ids = self.read_user_buffer(
                        request.set_connectors_ptr,
                        request.count_connectors as usize * core::mem::size_of::<u32>(),
                    )?;
                    if connector_ids.len() >= 4 {
                        let connector_id = u32::from_ne_bytes(
                            connector_ids[..4].try_into().map_err(|_| SysErr::Fault)?,
                        );
                        if device.get_connector(connector_id).is_none() {
                            return Err(SysErr::NoEnt);
                        }
                    }
                }
                if request.fb_id != 0 {
                    device.set_crtc(request.fb_id).map_err(SysErr::from)?;
                }
            }
            drm_abi::DRM_IOCTL_WAIT_VBLANK => {
                let request = drm_abi::DrmWaitVBlank::from_bytes(&bytes).ok_or(SysErr::Fault)?;
                let reply = device.wait_vblank(request).map_err(SysErr::from)?;
                if !reply.write_to_bytes(&mut bytes) {
                    return Err(SysErr::Fault);
                }
            }
            drm_abi::DRM_IOCTL_MODE_PAGE_FLIP => {
                let request =
                    drm_abi::DrmModeCrtcPageFlip::from_bytes(&bytes).ok_or(SysErr::Fault)?;
                if request.reserved != 0 {
                    return Err(SysErr::Inval);
                }
                if request.crtc_id
                    != device
                        .get_crtc(request.crtc_id)
                        .ok_or(SysErr::NoEnt)?
                        .crtc_id
                {
                    return Err(SysErr::NoEnt);
                }
                device
                    .page_flip(request.fb_id, request.flags, request.user_data)
                    .map_err(SysErr::from)?;
            }
            drm_abi::DRM_IOCTL_MODE_DIRTYFB => {
                let request =
                    drm_abi::DrmModeFbDirtyCmd::from_bytes(&bytes).ok_or(SysErr::Fault)?;
                let _ = request.color;
                let _ = request.num_clips;
                let _ = request.clips_ptr;
                device
                    .dirty_framebuffer(request.fb_id, request.flags)
                    .map_err(SysErr::from)?;
            }
            _ => return Err(SysErr::NoTty),
        }

        if size != 0 && (dir & drm_abi::IOC_READ) != 0 {
            self.write_user_buffer(argument, &bytes)?;
        }
        Ok(0)
    }

    fn read_drm_u32_array(
        &mut self,
        address: u64,
        count: usize,
    ) -> SysResult<alloc::vec::Vec<u32>> {
        if count == 0 {
            return Ok(alloc::vec::Vec::new());
        }
        if address == 0 {
            return Err(SysErr::Fault);
        }
        decode_u32_array(
            &self.syscall_read_user_exact_buffer(address, count * core::mem::size_of::<u32>())?,
        )
    }

    fn read_drm_u64_array(
        &mut self,
        address: u64,
        count: usize,
    ) -> SysResult<alloc::vec::Vec<u64>> {
        if count == 0 {
            return Ok(alloc::vec::Vec::new());
        }
        if address == 0 {
            return Err(SysErr::Fault);
        }
        decode_u64_array(
            &self.syscall_read_user_exact_buffer(address, count * core::mem::size_of::<u64>())?,
        )
    }
}

struct DrmUserWriter<'ctx, 'proc, S: ProcessServices> {
    ctx: &'ctx mut ProcessSyscallContext<'proc, S>,
}

impl<'ctx, 'proc, S: ProcessServices> DrmUserWriter<'ctx, 'proc, S> {
    fn new(ctx: &'ctx mut ProcessSyscallContext<'proc, S>) -> Self {
        Self { ctx }
    }

    fn write_bytes(&mut self, address: u64, capacity: usize, bytes: &[u8]) -> SysResult<()> {
        if address == 0 || capacity == 0 {
            return Ok(());
        }
        self.ctx
            .write_user_buffer(address, &bytes[..bytes.len().min(capacity)])
    }

    fn write_u32s(&mut self, address: u64, capacity: usize, values: &[u32]) -> SysResult<()> {
        if address == 0 || capacity == 0 {
            return Ok(());
        }
        let bytes = drm_abi::encode_u32_array(&values[..values.len().min(capacity)]);
        self.ctx.write_user_buffer(address, &bytes)
    }

    fn write_u64s(&mut self, address: u64, capacity: usize, values: &[u64]) -> SysResult<()> {
        if address == 0 || capacity == 0 {
            return Ok(());
        }
        let bytes = drm_abi::encode_u64_array(&values[..values.len().min(capacity)]);
        self.ctx.write_user_buffer(address, &bytes)
    }

    fn write_modes(
        &mut self,
        address: u64,
        capacity: usize,
        values: &[DrmModeInfo],
    ) -> SysResult<()> {
        if address == 0 || capacity == 0 {
            return Ok(());
        }
        let bytes = drm_abi::encode_modes(&values[..values.len().min(capacity)]);
        self.ctx.write_user_buffer(address, &bytes)
    }

    fn write_property_enums(
        &mut self,
        address: u64,
        capacity: usize,
        values: &[aether_drivers::drm::DrmPropertyEnumValue],
    ) -> SysResult<()> {
        if address == 0 || capacity == 0 {
            return Ok(());
        }
        let bytes = drm_abi::encode_property_enums(&values[..values.len().min(capacity)]);
        self.ctx.write_user_buffer(address, &bytes)
    }
}

fn decode_u32_array(bytes: &[u8]) -> SysResult<alloc::vec::Vec<u32>> {
    if bytes.len() % core::mem::size_of::<u32>() != 0 {
        return Err(SysErr::Fault);
    }
    bytes
        .chunks_exact(core::mem::size_of::<u32>())
        .map(|chunk| {
            Ok(u32::from_ne_bytes(
                chunk.try_into().map_err(|_| SysErr::Fault)?,
            ))
        })
        .collect()
}

fn decode_u64_array(bytes: &[u8]) -> SysResult<alloc::vec::Vec<u64>> {
    if bytes.len() % core::mem::size_of::<u64>() != 0 {
        return Err(SysErr::Fault);
    }
    bytes
        .chunks_exact(core::mem::size_of::<u64>())
        .map(|chunk| {
            Ok(u64::from_ne_bytes(
                chunk.try_into().map_err(|_| SysErr::Fault)?,
            ))
        })
        .collect()
}
