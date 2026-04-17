extern crate alloc;

mod codes;

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use alloc::{format, vec};
use core::any::Any;
use core::mem::size_of;
use core::sync::atomic::{AtomicI32, AtomicU64, AtomicUsize, Ordering};

use aether_device::{DeviceClass, DeviceMetadata, DeviceNode, KernelDevice, SysfsEntry};
use aether_frame::interrupt::timer;
use aether_frame::libs::spin::SpinLock;
use aether_vfs::{
    FileNode, FileOperations, FsError, FsResult, IoctlResponse, NodeRef, PollEvents,
    SharedWaitListener, WaitQueue,
};

pub use self::codes::*;

pub const INPUT_MAJOR: u16 = 13;
pub const INPUT_EVENT_MINOR_BASE: u16 = 0;
pub const EVDEV_VERSION: i32 = 0x010001;

const INPUT_EVENT_QUEUE_CAPACITY: usize = 1024;
const EVDEV_IOC_TYPE: u64 = b'E' as u64;
const IOC_NRBITS: u64 = 8;
const IOC_TYPEBITS: u64 = 8;
const IOC_SIZEBITS: u64 = 14;
const IOC_NRSHIFT: u64 = 0;
const IOC_TYPESHIFT: u64 = IOC_NRSHIFT + IOC_NRBITS;
const IOC_SIZESHIFT: u64 = IOC_TYPESHIFT + IOC_TYPEBITS;

static NEXT_EVENT_INDEX: AtomicUsize = AtomicUsize::new(0);
static NEXT_INPUT_INDEX: AtomicUsize = AtomicUsize::new(0);
static LISTENERS: SpinLock<Vec<Weak<dyn InputEventSink>>> = SpinLock::new(Vec::new());

pub const fn input_bitmap_bytes(nr: usize) -> usize {
    nr.div_ceil(8)
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LinuxInputId {
    pub bustype: u16,
    pub vendor: u16,
    pub product: u16,
    pub version: u16,
}

impl LinuxInputId {
    pub fn to_bytes(self) -> [u8; size_of::<Self>()] {
        let mut bytes = [0u8; size_of::<Self>()];
        bytes[0..2].copy_from_slice(&self.bustype.to_ne_bytes());
        bytes[2..4].copy_from_slice(&self.vendor.to_ne_bytes());
        bytes[4..6].copy_from_slice(&self.product.to_ne_bytes());
        bytes[6..8].copy_from_slice(&self.version.to_ne_bytes());
        bytes
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LinuxInputAbsInfo {
    pub value: i32,
    pub minimum: i32,
    pub maximum: i32,
    pub fuzz: i32,
    pub flat: i32,
    pub resolution: i32,
}

impl LinuxInputAbsInfo {
    pub fn to_bytes(self) -> [u8; size_of::<Self>()] {
        let mut bytes = [0u8; size_of::<Self>()];
        bytes[0..4].copy_from_slice(&self.value.to_ne_bytes());
        bytes[4..8].copy_from_slice(&self.minimum.to_ne_bytes());
        bytes[8..12].copy_from_slice(&self.maximum.to_ne_bytes());
        bytes[12..16].copy_from_slice(&self.fuzz.to_ne_bytes());
        bytes[16..20].copy_from_slice(&self.flat.to_ne_bytes());
        bytes[20..24].copy_from_slice(&self.resolution.to_ne_bytes());
        bytes
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LinuxInputEvent {
    pub sec: i64,
    pub usec: i64,
    pub type_: u16,
    pub code: u16,
    pub value: i32,
}

impl LinuxInputEvent {
    pub fn now(type_: u16, code: u16, value: i32, clock_id: i32) -> Self {
        let nanos = timer::nanos_since_boot();
        let (sec, subsec_nanos) = if clock_id == linux_clock_realtime() {
            let (sec, nanos) = timer::unix_time_nanos();
            (sec, nanos)
        } else {
            ((nanos / 1_000_000_000) as i64, nanos % 1_000_000_000)
        };
        Self {
            sec,
            usec: (subsec_nanos / 1_000) as i64,
            type_,
            code,
            value,
        }
    }

    pub fn to_bytes(self) -> [u8; size_of::<Self>()] {
        let mut bytes = [0u8; size_of::<Self>()];
        bytes[0..8].copy_from_slice(&self.sec.to_ne_bytes());
        bytes[8..16].copy_from_slice(&self.usec.to_ne_bytes());
        bytes[16..18].copy_from_slice(&self.type_.to_ne_bytes());
        bytes[18..20].copy_from_slice(&self.code.to_ne_bytes());
        bytes[20..24].copy_from_slice(&self.value.to_ne_bytes());
        bytes
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputTopology {
    Virtual,
    PlatformDevice {
        device: String,
        child: Option<String>,
    },
}

#[derive(Clone)]
pub struct InputDeviceDescriptor {
    pub name: String,
    pub phys: String,
    pub uniq: String,
    pub input_id: LinuxInputId,
    pub properties: u64,
    pub evbit: [u8; input_bitmap_bytes(EV_CNT)],
    pub keybit: [u8; input_bitmap_bytes(KEY_CNT)],
    pub relbit: [u8; input_bitmap_bytes(REL_CNT)],
    pub absbit: [u8; input_bitmap_bytes(ABS_CNT)],
    pub absinfo: [LinuxInputAbsInfo; ABS_CNT],
}

impl InputDeviceDescriptor {
    pub fn new(name: impl Into<String>) -> Self {
        let mut descriptor = Self {
            name: name.into(),
            phys: String::new(),
            uniq: String::new(),
            input_id: LinuxInputId::default(),
            properties: 0,
            evbit: [0; input_bitmap_bytes(EV_CNT)],
            keybit: [0; input_bitmap_bytes(KEY_CNT)],
            relbit: [0; input_bitmap_bytes(REL_CNT)],
            absbit: [0; input_bitmap_bytes(ABS_CNT)],
            absinfo: [LinuxInputAbsInfo::default(); ABS_CNT],
        };
        descriptor.set_event(EV_SYN);
        descriptor
    }

    pub fn with_phys(mut self, phys: impl Into<String>) -> Self {
        self.phys = phys.into();
        self
    }

    pub fn with_uniq(mut self, uniq: impl Into<String>) -> Self {
        self.uniq = uniq.into();
        self
    }

    pub fn with_input_id(mut self, input_id: LinuxInputId) -> Self {
        self.input_id = input_id;
        self
    }

    pub fn set_event(&mut self, type_: u16) {
        bitmap_set(&mut self.evbit, type_ as usize);
    }

    pub fn set_key(&mut self, code: u16) {
        self.set_event(EV_KEY);
        bitmap_set(&mut self.keybit, code as usize);
    }

    pub fn set_rel(&mut self, code: u16) {
        self.set_event(EV_REL);
        bitmap_set(&mut self.relbit, code as usize);
    }

    pub fn set_abs(&mut self, code: u16, minimum: i32, maximum: i32) {
        self.set_event(EV_ABS);
        bitmap_set(&mut self.absbit, code as usize);
        self.absinfo[code as usize].minimum = minimum;
        self.absinfo[code as usize].maximum = maximum;
    }

    pub fn set_property(&mut self, property: u16) {
        if property as usize >= u64::BITS as usize {
            return;
        }
        self.properties |= 1u64 << property;
    }

    pub fn supports_key(&self, code: u16) -> bool {
        bitmap_test(&self.keybit, code as usize)
    }
}

pub trait InputEventSink: Send + Sync {
    fn on_input_event(&self, device: &InputDevice, event: LinuxInputEvent);
}

pub fn register_input_sink(listener: Arc<dyn InputEventSink>) {
    let mut listeners = LISTENERS.lock();
    listeners.retain(|entry| entry.upgrade().is_some());
    listeners.push(Arc::downgrade(&listener));
}

pub fn next_event_index() -> usize {
    NEXT_EVENT_INDEX.fetch_add(1, Ordering::AcqRel)
}

pub fn next_input_index() -> usize {
    NEXT_INPUT_INDEX.fetch_add(1, Ordering::AcqRel)
}

struct InputDeviceState {
    queue: VecDeque<LinuxInputEvent>,
}

struct InputDeviceInner {
    event_index: usize,
    input_index: usize,
    minor: u16,
    topology: InputTopology,
    descriptor: InputDeviceDescriptor,
    clock_id: AtomicI32,
    state: SpinLock<InputDeviceState>,
    version: AtomicU64,
    waiters: WaitQueue,
}

pub struct EvdevFile {
    device: Arc<InputDeviceInner>,
}

#[derive(Clone)]
pub struct InputDevice {
    inner: Arc<InputDeviceInner>,
    file: Arc<EvdevFile>,
}

impl InputDevice {
    pub fn new(topology: InputTopology, descriptor: InputDeviceDescriptor) -> Arc<Self> {
        let event_index = next_event_index();
        let input_index = next_input_index();
        let minor = INPUT_EVENT_MINOR_BASE + event_index as u16;
        let inner = Arc::new(InputDeviceInner {
            event_index,
            input_index,
            minor,
            topology,
            descriptor,
            clock_id: AtomicI32::new(linux_clock_monotonic()),
            state: SpinLock::new(InputDeviceState {
                queue: VecDeque::with_capacity(INPUT_EVENT_QUEUE_CAPACITY),
            }),
            version: AtomicU64::new(1),
            waiters: WaitQueue::new(),
        });
        Arc::new(Self {
            file: Arc::new(EvdevFile {
                device: inner.clone(),
            }),
            inner,
        })
    }

    pub fn name(&self) -> &str {
        &self.inner.descriptor.name
    }

    pub fn input_index(&self) -> usize {
        self.inner.input_index
    }

    pub fn event_index(&self) -> usize {
        self.inner.event_index
    }

    pub fn topology(&self) -> &InputTopology {
        &self.inner.topology
    }

    pub fn descriptor(&self) -> &InputDeviceDescriptor {
        &self.inner.descriptor
    }

    pub fn is_keyboard(&self) -> bool {
        self.inner.descriptor.supports_key(KEY_A)
            || self.inner.descriptor.supports_key(KEY_ENTER)
            || self.inner.descriptor.supports_key(KEY_SPACE)
    }

    pub fn emit(&self, type_: u16, code: u16, value: i32) {
        let event = LinuxInputEvent::now(type_, code, value, self.inner.clock_id());
        self.emit_event(event);
    }

    pub fn emit_event(&self, event: LinuxInputEvent) {
        {
            let mut state = self.inner.state.lock();
            if state.queue.len() >= INPUT_EVENT_QUEUE_CAPACITY {
                let _ = state.queue.pop_front();
            }
            state.queue.push_back(event);
        }
        self.inner.bump_version();
        self.inner.waiters.notify(PollEvents::READ);
        notify_listeners(self, event);
    }
}

impl InputDeviceInner {
    fn clock_id(&self) -> i32 {
        self.clock_id.load(Ordering::Acquire)
    }

    fn set_clock_id(&self, clock_id: i32) {
        self.clock_id.store(clock_id, Ordering::Release);
        self.bump_version();
    }

    fn bump_version(&self) {
        let _ = self.version.fetch_add(1, Ordering::AcqRel);
    }

    fn sysfs_parent_under_devices(&self) -> String {
        match &self.topology {
            InputTopology::Virtual => format!("virtual/input/input{}", self.input_index),
            InputTopology::PlatformDevice { device, child } => match child {
                Some(child) => format!("platform/{device}/{child}/input/input{}", self.input_index),
                None => format!("platform/{device}/input/input{}", self.input_index),
            },
        }
    }

    fn event_dir_under_devices(&self) -> String {
        format!(
            "{}/event{}",
            self.sysfs_parent_under_devices(),
            self.event_index
        )
    }

    fn sysfs_entries(&self) -> Vec<SysfsEntry> {
        let mut entries = Vec::new();
        let parent = self.sysfs_parent_under_devices();
        let event_path = self.event_dir_under_devices();
        let parent_node = format!("devices/{parent}");
        let id_dir = format!("{parent_node}/id");
        let caps_dir = format!("{parent_node}/capabilities");
        let mut directories = vec![parent_node.clone(), id_dir.clone(), caps_dir.clone()];
        if let InputTopology::PlatformDevice { device, child } = &self.topology {
            directories.push(String::from("devices/platform"));
            directories.push(format!("devices/platform/{device}"));
            if let Some(child) = child {
                directories.push(format!("devices/platform/{device}/{child}"));
                directories.push(format!("devices/platform/{device}/{child}/input"));
            } else {
                directories.push(format!("devices/platform/{device}/input"));
            }
        }
        for dir in directories {
            entries.push(SysfsEntry::directory(dir, 0o040755));
        }

        entries.push(SysfsEntry::symlink(
            format!("class/input/input{}", self.input_index),
            format!("../../devices/{parent}"),
        ));
        entries.push(SysfsEntry::symlink(
            format!("devices/{event_path}/device"),
            "..",
        ));
        entries.push(SysfsEntry::symlink(
            format!("{parent_node}/subsystem"),
            relative_sysfs_target(parent.as_str(), "class/input"),
        ));
        entries.push(SysfsEntry::file(
            format!("{parent_node}/name"),
            0o100444,
            c_string_bytes(self.descriptor.name.as_str()),
        ));
        entries.push(SysfsEntry::file(
            format!("{parent_node}/phys"),
            0o100444,
            c_string_bytes(self.descriptor.phys.as_str()),
        ));
        entries.push(SysfsEntry::file(
            format!("{parent_node}/uniq"),
            0o100444,
            c_string_bytes(self.descriptor.uniq.as_str()),
        ));
        entries.push(SysfsEntry::file(
            format!("{parent_node}/properties"),
            0o100444,
            format!("{}\n", self.descriptor.properties).into_bytes(),
        ));
        entries.push(SysfsEntry::file(
            format!("{parent_node}/modalias"),
            0o100444,
            format!(
                "input:b{:04X}v{:04X}p{:04X}e{:04X}\n",
                self.descriptor.input_id.bustype,
                self.descriptor.input_id.vendor,
                self.descriptor.input_id.product,
                self.descriptor.input_id.version
            )
            .into_bytes(),
        ));
        entries.push(SysfsEntry::file(
            format!("{parent_node}/uevent"),
            0o100444,
            render_input_uevent(self),
        ));
        entries.push(SysfsEntry::file(
            format!("{id_dir}/bustype"),
            0o100444,
            format!("{:04x}\n", self.descriptor.input_id.bustype).into_bytes(),
        ));
        entries.push(SysfsEntry::file(
            format!("{id_dir}/vendor"),
            0o100444,
            format!("{:04x}\n", self.descriptor.input_id.vendor).into_bytes(),
        ));
        entries.push(SysfsEntry::file(
            format!("{id_dir}/product"),
            0o100444,
            format!("{:04x}\n", self.descriptor.input_id.product).into_bytes(),
        ));
        entries.push(SysfsEntry::file(
            format!("{id_dir}/version"),
            0o100444,
            format!("{:04x}\n", self.descriptor.input_id.version).into_bytes(),
        ));
        for (name, value) in [
            ("ev", bitmap_to_string(&self.descriptor.evbit)),
            ("key", bitmap_to_string(&self.descriptor.keybit)),
            ("rel", bitmap_to_string(&self.descriptor.relbit)),
            ("abs", bitmap_to_string(&self.descriptor.absbit)),
            ("msc", String::from("0\n")),
            ("led", String::from("0\n")),
            ("snd", String::from("0\n")),
            ("sw", String::from("0\n")),
            ("ff", String::from("0\n")),
        ] {
            entries.push(SysfsEntry::file(
                format!("{caps_dir}/{name}"),
                0o100444,
                value.into_bytes(),
            ));
        }
        entries
    }
}

impl EvdevFile {
    pub fn set_clock_id(&self, clock_id: i32) -> FsResult<()> {
        match clock_id {
            x if x == linux_clock_monotonic() || x == linux_clock_realtime() => {
                self.device.set_clock_id(clock_id);
                Ok(())
            }
            _ => Err(FsError::InvalidInput),
        }
    }

    fn ioctl_response(&self, request: u64) -> FsResult<IoctlResponse> {
        if ioctl_type(request) != EVDEV_IOC_TYPE {
            return Err(FsError::Unsupported);
        }
        let nr = ioctl_nr(request);
        let size = ioctl_size(request) as usize;
        let descriptor = &self.device.descriptor;

        match nr {
            0x01 => {
                let bytes = EVDEV_VERSION.to_ne_bytes();
                let len = size.min(bytes.len());
                Ok(IoctlResponse::DataValue(bytes[..len].to_vec(), len as u64))
            }
            0x02 => {
                let bytes = descriptor.input_id.to_bytes();
                Ok(IoctlResponse::DataValue(
                    bytes[..size.min(bytes.len())].to_vec(),
                    size.min(bytes.len()) as u64,
                ))
            }
            0x03 => {
                let repeat = [500u32.to_ne_bytes(), 33u32.to_ne_bytes()].concat();
                Ok(IoctlResponse::DataValue(
                    repeat[..size.min(repeat.len())].to_vec(),
                    size.min(repeat.len()) as u64,
                ))
            }
            0x06 => Ok(ioctl_string(descriptor.name.as_str(), size)),
            0x07 => Ok(ioctl_string(descriptor.phys.as_str(), size)),
            0x08 => Ok(ioctl_string(descriptor.uniq.as_str(), size)),
            0x09 => Ok(ioctl_properties(descriptor.properties, size)),
            0x18 => Ok(IoctlResponse::DataValue(vec![0; size], size as u64)),
            0x19 | 0x1b => {
                let zero = 0usize.to_ne_bytes();
                let bytes = zero[..size.min(zero.len())].to_vec();
                Ok(IoctlResponse::DataValue(bytes.clone(), bytes.len() as u64))
            }
            0x20 => Ok(ioctl_bitmap(&descriptor.evbit, size)),
            value if value == (0x20 + EV_KEY as u64) => Ok(ioctl_bitmap(&descriptor.keybit, size)),
            value if value == (0x20 + EV_REL as u64) => Ok(ioctl_bitmap(&descriptor.relbit, size)),
            value if value == (0x20 + EV_ABS as u64) => Ok(ioctl_bitmap(&descriptor.absbit, size)),
            value
                if matches!(
                    value as u16,
                    x if x == 0x20 + EV_SW || x == 0x20 + EV_MSC || x == 0x20 + EV_SND || x == 0x20 + EV_LED
                ) =>
            {
                Ok(IoctlResponse::DataValue(vec![0; size], size as u64))
            }
            value if value == (0x20 + EV_FF as u64) => {
                let len = size.min(16);
                Ok(IoctlResponse::DataValue(vec![0; len], len as u64))
            }
            value if (0x40..0x40 + ABS_CNT as u64).contains(&value) => {
                let index = (value - 0x40) as usize;
                let bytes = descriptor.absinfo[index].to_bytes();
                Ok(IoctlResponse::DataValue(
                    bytes[..size.min(bytes.len())].to_vec(),
                    size.min(bytes.len()) as u64,
                ))
            }
            0x90 | 0x91 => Ok(IoctlResponse::from_value(0)),
            _ => Err(FsError::Unsupported),
        }
    }
}

impl FileOperations for EvdevFile {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn read(&self, _offset: usize, buffer: &mut [u8]) -> FsResult<usize> {
        let event_size = size_of::<LinuxInputEvent>();
        if buffer.len() < event_size {
            return Err(FsError::InvalidInput);
        }

        let mut state = self.device.state.lock();
        if state.queue.is_empty() {
            return Err(FsError::WouldBlock);
        }

        let mut written = 0usize;
        while written + event_size <= buffer.len() {
            let Some(event) = state.queue.pop_front() else {
                break;
            };
            buffer[written..written + event_size].copy_from_slice(&event.to_bytes());
            written += event_size;
        }
        Ok(written)
    }

    fn ioctl(&self, command: u64, _argument: u64) -> FsResult<IoctlResponse> {
        self.ioctl_response(command)
    }

    fn poll(&self, events: PollEvents) -> FsResult<PollEvents> {
        let mut ready = PollEvents::empty();
        let state = self.device.state.lock();
        if events.contains(PollEvents::READ) && !state.queue.is_empty() {
            ready = ready | PollEvents::READ;
        }
        Ok(ready)
    }

    fn wait_token(&self) -> u64 {
        self.device.version.load(Ordering::Acquire)
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
}

impl KernelDevice for InputDevice {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn metadata(&self) -> DeviceMetadata {
        DeviceMetadata::new(
            format!("event{}", self.inner.event_index),
            DeviceClass::Input,
            INPUT_MAJOR,
            self.inner.minor,
        )
    }

    fn nodes(&self) -> Vec<DeviceNode> {
        let name = format!("event{}", self.inner.event_index);
        let node: NodeRef = FileNode::new_char_device(
            name.clone(),
            u32::from(INPUT_MAJOR),
            u32::from(self.inner.minor),
            self.file.clone(),
        );
        vec![DeviceNode::new(format!("input/{name}"), node)]
    }

    fn sysfs_devpath_under_devices(&self) -> Option<String> {
        Some(self.inner.event_dir_under_devices())
    }

    fn sysfs_entries(&self) -> Vec<SysfsEntry> {
        self.inner.sysfs_entries()
    }

    fn uevent_fields(&self) -> Vec<String> {
        let mut fields = vec![String::from("ID_INPUT=1")];
        if self.is_keyboard() {
            fields.push(String::from("ID_INPUT_KEY=1"));
            fields.push(String::from("ID_INPUT_KEYBOARD=1"));
        } else if self.inner.descriptor.supports_key(BTN_LEFT)
            || self.inner.descriptor.supports_key(BTN_RIGHT)
            || self.inner.descriptor.supports_key(BTN_MIDDLE)
        {
            fields.push(String::from("ID_INPUT_MOUSE=1"));
        }
        fields
    }
}

fn notify_listeners(device: &InputDevice, event: LinuxInputEvent) {
    let listeners = {
        let mut guard = LISTENERS.lock();
        guard.retain(|entry| entry.upgrade().is_some());
        guard.iter().filter_map(Weak::upgrade).collect::<Vec<_>>()
    };
    for listener in listeners {
        listener.on_input_event(device, event);
    }
}

fn render_input_uevent(device: &InputDeviceInner) -> Vec<u8> {
    let descriptor = &device.descriptor;
    format!(
        "PRODUCT={:04X}/{:04X}/{:04X}/{:04X}\nNAME=\"{}\"\nPHYS={}\nPROP={:x}\nEV={}\nKEY={}\nREL={}\nABS={}\n",
        descriptor.input_id.bustype,
        descriptor.input_id.vendor,
        descriptor.input_id.product,
        descriptor.input_id.version,
        descriptor.name,
        descriptor.phys,
        descriptor.properties,
        bitmap_to_hex_word(&descriptor.evbit),
        bitmap_to_hex_word(&descriptor.keybit),
        bitmap_to_hex_word(&descriptor.relbit),
        bitmap_to_hex_word(&descriptor.absbit),
    )
    .into_bytes()
}

fn c_string_bytes(value: &str) -> Vec<u8> {
    format!("{value}\n").into_bytes()
}

fn ioctl_string(value: &str, size: usize) -> IoctlResponse {
    let mut bytes = value.as_bytes().to_vec();
    bytes.push(0);
    let copied = size.min(bytes.len());
    IoctlResponse::DataValue(bytes[..copied].to_vec(), copied as u64)
}

fn ioctl_bitmap(bitmap: &[u8], size: usize) -> IoctlResponse {
    let copied = size.min(bitmap.len());
    IoctlResponse::DataValue(bitmap[..copied].to_vec(), copied as u64)
}

fn ioctl_properties(properties: u64, size: usize) -> IoctlResponse {
    let mut bitmap = [0u8; input_bitmap_bytes(INPUT_PROP_CNT)];
    for (index, byte) in bitmap.iter_mut().enumerate() {
        *byte = ((properties >> (index * 8)) & 0xff) as u8;
    }
    let copied = size.min(bitmap.len());
    IoctlResponse::DataValue(bitmap[..copied].to_vec(), copied as u64)
}

fn bitmap_set(bitmap: &mut [u8], bit: usize) {
    if bit / 8 >= bitmap.len() {
        return;
    }
    bitmap[bit / 8] |= 1u8 << (bit % 8);
}

fn bitmap_test(bitmap: &[u8], bit: usize) -> bool {
    (bit / 8) < bitmap.len() && (bitmap[bit / 8] & (1u8 << (bit % 8))) != 0
}

fn bitmap_to_hex_word(bitmap: &[u8]) -> String {
    let mut value = 0u64;
    for (index, byte) in bitmap.iter().copied().enumerate().take(size_of::<u64>()) {
        value |= (byte as u64) << (index * 8);
    }
    format!("{value:x}")
}

fn bitmap_to_string(bitmap: &[u8]) -> String {
    let word_bytes = size_of::<usize>();
    let mut last = bitmap.len().div_ceil(word_bytes);
    while last > 1 {
        let start = (last - 1) * word_bytes;
        let end = bitmap.len().min(start + word_bytes);
        if bitmap[start..end].iter().any(|byte| *byte != 0) {
            break;
        }
        last -= 1;
    }

    let mut out = String::new();
    for word in (0..last).rev() {
        let start = word * word_bytes;
        let end = bitmap.len().min(start + word_bytes);
        let mut value = 0usize;
        for (index, byte) in bitmap[start..end].iter().copied().enumerate() {
            value |= (byte as usize) << (index * 8);
        }
        if out.is_empty() {
            out.push_str(format!("{value:x}").as_str());
        } else {
            out.push_str(format!(" {value:0width$x}", width = word_bytes * 2).as_str());
        }
    }
    out.push('\n');
    out
}

fn relative_sysfs_target(path_under_devices: &str, target_under_sys: &str) -> String {
    let depth = path_under_devices
        .split('/')
        .filter(|component| !component.is_empty())
        .count()
        + 1;
    let mut relative = String::new();
    for _ in 0..depth {
        relative.push_str("../");
    }
    relative.push_str(target_under_sys.trim_start_matches('/'));
    relative
}

fn ioctl_nr(request: u64) -> u64 {
    (request >> IOC_NRSHIFT) & ((1 << IOC_NRBITS) - 1)
}

fn ioctl_type(request: u64) -> u64 {
    (request >> IOC_TYPESHIFT) & ((1 << IOC_TYPEBITS) - 1)
}

fn ioctl_size(request: u64) -> u64 {
    (request >> IOC_SIZESHIFT) & ((1 << IOC_SIZEBITS) - 1)
}

const fn linux_clock_monotonic() -> i32 {
    1
}

const fn linux_clock_realtime() -> i32 {
    0
}
