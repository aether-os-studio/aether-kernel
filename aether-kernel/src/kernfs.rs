extern crate alloc;

use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;

use aether_device::{
    DeviceClass, DeviceNamespace, DeviceNode, KernelDevice, SysfsEntry, SysfsEntryKind,
};
use aether_frame::bus::pci::PciDeviceInfo;
use aether_frame::time;
use aether_fs::pseudo::{BytesGenerator, generated_bytes_file, generated_text_file};
use aether_tmpfs as tmpfs;
use aether_vfs::{FileNode, FileOperations, FsError, FsResult, NodeKind, NodeRef, Vfs};

use crate::fs::LinuxUtsName;
use crate::net::{
    current_kobject_uevent_seqnum, publish_kobject_uevent, reserve_kobject_uevent_seqnum,
};

pub struct KernelResourceRegistry {
    proc_root: NodeRef,
    sys_root: NodeRef,
}

pub struct SysfsResource<'a> {
    pub class: DeviceClass,
    pub name: &'a str,
    pub major: u16,
    pub minor: u16,
    pub nodes: &'a [DeviceNode],
}

pub struct SysfsBus {
    pub name: String,
    pub attributes: Vec<SysfsAttribute>,
}

pub struct SysfsDevice {
    pub name: String,
    pub path: String,
    pub attributes: Vec<SysfsAttribute>,
    pub bin_attributes: Vec<SysfsBinAttribute>,
    pub links: Vec<SysfsLink>,
}

pub struct SysfsAttribute {
    pub name: String,
    pub mode: u32,
    source: SysfsFileSource,
}

pub struct SysfsBinAttribute {
    pub name: String,
    pub mode: u32,
    source: SysfsFileSource,
}

pub struct SysfsLink {
    pub name: String,
    pub target: String,
}

enum SysfsFileSource {
    Static(Vec<u8>),
    Generated(BytesGenerator),
}

struct SysfsUeventFile {
    content: Vec<u8>,
    class: DeviceClass,
    name: String,
    devpath: String,
    subsystem: String,
    major: u16,
    minor: u16,
    nodes: Vec<DeviceNode>,
    extra_fields: Vec<String>,
}

impl SysfsBus {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            attributes: Vec::new(),
        }
    }
}

impl SysfsDevice {
    pub fn new(name: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            path: path.into(),
            attributes: Vec::new(),
            bin_attributes: Vec::new(),
            links: Vec::new(),
        }
    }

    pub fn with_attribute(mut self, attribute: SysfsAttribute) -> Self {
        self.attributes.push(attribute);
        self
    }

    pub fn with_bin_attribute(mut self, attribute: SysfsBinAttribute) -> Self {
        self.bin_attributes.push(attribute);
        self
    }
}

impl SysfsAttribute {
    pub fn text(name: impl Into<String>, value: impl Into<Vec<u8>>) -> Self {
        Self {
            name: name.into(),
            mode: 0o100444,
            source: SysfsFileSource::Static(value.into()),
        }
    }
}

impl SysfsBinAttribute {
    pub fn generated_bytes(name: impl Into<String>, mode: u32, generator: BytesGenerator) -> Self {
        Self {
            name: name.into(),
            mode,
            source: SysfsFileSource::Generated(generator),
        }
    }
}

impl FileOperations for SysfsUeventFile {
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn read(&self, offset: usize, buffer: &mut [u8]) -> FsResult<usize> {
        if offset >= self.content.len() {
            return Ok(0);
        }
        let len = buffer.len().min(self.content.len() - offset);
        buffer[..len].copy_from_slice(&self.content[offset..offset + len]);
        Ok(len)
    }

    fn write(&self, _offset: usize, buffer: &[u8]) -> FsResult<usize> {
        let action = core::str::from_utf8(buffer)
            .map_err(|_| FsError::InvalidInput)?
            .trim_matches(char::from(0))
            .trim();
        if action.is_empty() {
            return Ok(buffer.len());
        }
        let seqnum = reserve_kobject_uevent_seqnum();
        publish_kobject_uevent(
            render_uevent_message(
                action,
                self.class,
                self.name.as_str(),
                self.devpath.as_str(),
                self.subsystem.as_str(),
                self.major,
                self.minor,
                self.nodes.as_slice(),
                seqnum,
                self.extra_fields.as_slice(),
            )
            .as_slice(),
        );
        Ok(buffer.len())
    }

    fn size(&self) -> usize {
        self.content.len()
    }
}

impl KernelResourceRegistry {
    pub fn new(
        proc_root: NodeRef,
        sys_root: NodeRef,
        command_line: Option<&str>,
        filesystems: &[&str],
    ) -> FsResult<Self> {
        let registry = Self {
            proc_root,
            sys_root,
        };
        registry.install_procfs(command_line, filesystems)?;
        registry.install_sysfs_skeleton()?;
        Ok(registry)
    }

    pub fn register_device(
        &self,
        vfs: &Vfs,
        dev_namespace: &DeviceNamespace,
        device: Arc<dyn KernelDevice>,
    ) -> FsResult<()> {
        let metadata = device.metadata();
        let nodes = device.nodes();
        let uevent_fields = device.uevent_fields();

        for node in &nodes {
            dev_namespace.install(vfs, node.path.as_str(), node.node.clone())?;
        }

        self.register_sysfs_resource(
            SysfsResource {
                class: metadata.class,
                name: &metadata.name,
                major: metadata.major,
                minor: metadata.minor,
                nodes: &nodes,
            },
            device.sysfs_devpath_under_devices(),
            uevent_fields.as_slice(),
        )?;

        for entry in device.sysfs_entries() {
            self.install_sysfs_entry(&entry)?;
        }

        Ok(())
    }

    fn install_procfs(&self, command_line: Option<&str>, filesystems: &[&str]) -> FsResult<()> {
        let sys_dir = ensure_dir(&self.proc_root, "sys", 0o040755)?;
        let kernel_dir = ensure_dir(&sys_dir, "kernel", 0o040755)?;
        let uts = LinuxUtsName::linux_x86_64();

        if let Some(command_line) = command_line {
            install_child(
                &self.proc_root,
                tmpfs::file("cmdline", command_line.as_bytes()),
            )?;
        }

        install_child(
            &self.proc_root,
            tmpfs::file("filesystems", render_filesystems(filesystems).as_bytes()),
        )?;
        install_child(
            &self.proc_root,
            generated_text_file("uptime", 0o100444, Arc::new(render_uptime)),
        )?;
        install_child(
            &kernel_dir,
            tmpfs::file(
                "ostype",
                c_string_file(field_string(&uts.sysname).as_str()).as_slice(),
            ),
        )?;
        install_child(
            &kernel_dir,
            tmpfs::file(
                "osrelease",
                c_string_file(field_string(&uts.release).as_str()).as_slice(),
            ),
        )?;
        install_child(
            &kernel_dir,
            tmpfs::file(
                "version",
                c_string_file(field_string(&uts.version).as_str()).as_slice(),
            ),
        )?;
        install_child(
            &kernel_dir,
            tmpfs::file(
                "hostname",
                c_string_file(field_string(&uts.nodename).as_str()).as_slice(),
            ),
        )?;
        install_child(
            &kernel_dir,
            tmpfs::file(
                "domainname",
                c_string_file(field_string(&uts.domainname).as_str()).as_slice(),
            ),
        )?;

        // /proc/<pid>, /proc/self and mount-namespace aware proc views need a live process
        // snapshot provider. Wire those in after the process manager exposes a stable read API.
        Ok(())
    }

    fn install_sysfs_skeleton(&self) -> FsResult<()> {
        ensure_dir(&self.sys_root, "block", 0o040755)?;
        ensure_dir(&self.sys_root, "bus", 0o040755)?;
        ensure_dir(&self.sys_root, "class", 0o040755)?;
        ensure_dir(&self.sys_root, "dev", 0o040755)?;
        ensure_dir(&self.sys_root, "devices", 0o040755)?;
        ensure_dir(&self.sys_root, "fs", 0o040755)?;
        let kernel_dir = ensure_dir(&self.sys_root, "kernel", 0o040755)?;
        ensure_dir(&self.sys_root, "module", 0o040755)?;

        install_or_replace_child(
            &kernel_dir,
            generated_text_file("uevent_seqnum", 0o100444, Arc::new(render_uevent_seqnum)),
        )?;
        install_or_replace_child(&kernel_dir, tmpfs::file("uevent_helper", b"\n"))?;

        let dev_dir = ensure_dir(&self.sys_root, "dev", 0o040755)?;
        ensure_dir(&dev_dir, "block", 0o040755)?;
        ensure_dir(&dev_dir, "char", 0o040755)?;

        let devices_dir = ensure_dir(&self.sys_root, "devices", 0o040755)?;
        ensure_dir(&devices_dir, "virtual", 0o040755)?;
        Ok(())
    }

    pub fn register_sysfs_resource(
        &self,
        resource: SysfsResource<'_>,
        devpath_under_devices: Option<String>,
        uevent_fields: &[String],
    ) -> FsResult<()> {
        let class_name = class_name(resource.class);
        let class_dir = ensure_dir(
            &ensure_dir(&self.sys_root, "class", 0o040755)?,
            class_name,
            0o040755,
        )?;
        let device_path = devpath_under_devices
            .unwrap_or_else(|| alloc::format!("virtual/{class_name}/{}", resource.name));
        let devices_dir = ensure_dir(&self.sys_root, "devices", 0o040755)?;
        let device_dir = ensure_dir_path(&devices_dir, device_path.as_str(), 0o040755)?;
        let devpath = alloc::format!("/devices/{device_path}");
        let seqnum = reserve_kobject_uevent_seqnum();

        install_or_replace_child(
            &device_dir,
            tmpfs::file(
                "dev",
                alloc::format!("{}:{}\n", resource.major, resource.minor).as_bytes(),
            ),
        )?;
        install_or_replace_child(
            &device_dir,
            FileNode::new(
                "uevent",
                Arc::new(SysfsUeventFile {
                    content: render_uevent_file(
                        resource.class,
                        resource.name,
                        devpath.as_str(),
                        class_name,
                        resource.major,
                        resource.minor,
                        resource.nodes,
                        uevent_fields,
                    )
                    .into_bytes(),
                    class: resource.class,
                    name: resource.name.to_string(),
                    devpath: devpath.clone(),
                    subsystem: class_name.to_string(),
                    major: resource.major,
                    minor: resource.minor,
                    nodes: resource.nodes.to_vec(),
                    extra_fields: uevent_fields.to_vec(),
                }),
            ),
        )?;
        install_or_replace_child(
            &device_dir,
            tmpfs::symlink(
                "subsystem",
                relative_from_sys_devices(
                    device_path.as_str(),
                    alloc::format!("class/{class_name}").as_str(),
                ),
            ),
        )?;
        install_or_replace_child(
            &class_dir,
            tmpfs::symlink(resource.name, alloc::format!("../../devices/{device_path}")),
        )?;

        let dev_kind = match resource.class {
            DeviceClass::Block => "block",
            _ => "char",
        };
        let dev_map_dir = ensure_dir(
            &ensure_dir(&self.sys_root, "dev", 0o040755)?,
            dev_kind,
            0o040755,
        )?;
        install_or_replace_child(
            &dev_map_dir,
            tmpfs::symlink(
                alloc::format!("{}:{}", resource.major, resource.minor),
                alloc::format!("../../devices/{device_path}"),
            ),
        )?;

        publish_kobject_uevent(
            render_uevent_message(
                "add",
                resource.class,
                resource.name,
                devpath.as_str(),
                class_name,
                resource.major,
                resource.minor,
                resource.nodes,
                seqnum,
                uevent_fields,
            )
            .as_slice(),
        );

        Ok(())
    }

    fn install_sysfs_entry(&self, entry: &SysfsEntry) -> FsResult<()> {
        let trimmed = entry.path.trim_matches('/');
        if trimmed.is_empty() {
            return Err(FsError::InvalidInput);
        }
        let parent_path = trimmed
            .rsplit_once('/')
            .map(|(parent, _)| parent)
            .unwrap_or("");
        let name = trimmed
            .rsplit('/')
            .next()
            .filter(|name| !name.is_empty())
            .ok_or(FsError::InvalidInput)?;
        let parent = if parent_path.is_empty() {
            self.sys_root.clone()
        } else {
            ensure_dir_path(&self.sys_root, parent_path, 0o040755)?
        };

        match &entry.kind {
            SysfsEntryKind::Directory { mode } => {
                let _ = ensure_dir(&parent, name, *mode)?;
            }
            SysfsEntryKind::File { mode, bytes } => {
                install_or_replace_child(
                    &parent,
                    tmpfs::file_with_mode(name, bytes.as_slice(), *mode),
                )?;
            }
            SysfsEntryKind::Symlink { target } => {
                install_or_replace_child(&parent, tmpfs::symlink(name, target.as_str()))?;
            }
        }
        Ok(())
    }

    pub fn register_sysfs_bus(&self, bus: &SysfsBus) -> FsResult<()> {
        let bus_root = ensure_dir(
            &ensure_dir(&self.sys_root, "bus", 0o040755)?,
            &bus.name,
            0o040755,
        )?;
        ensure_dir(&bus_root, "devices", 0o040755)?;
        ensure_dir(&bus_root, "drivers", 0o040755)?;

        for attribute in &bus.attributes {
            install_sysfs_attribute(&bus_root, attribute)?;
        }

        Ok(())
    }

    pub fn register_sysfs_bus_device(&self, bus_name: &str, device: &SysfsDevice) -> FsResult<()> {
        self.register_sysfs_bus(&SysfsBus::new(bus_name))?;

        let devices_root = ensure_dir(&self.sys_root, "devices", 0o040755)?;
        let device_dir = ensure_dir_path(&devices_root, device.path.as_str(), 0o040755)?;

        for attribute in &device.attributes {
            install_sysfs_attribute(&device_dir, attribute)?;
        }
        for attribute in &device.bin_attributes {
            install_sysfs_bin_attribute(&device_dir, attribute)?;
        }
        for link in &device.links {
            install_or_replace_child(
                &device_dir,
                tmpfs::symlink(link.name.as_str(), link.target.as_str()),
            )?;
        }

        install_or_replace_child(
            &device_dir,
            tmpfs::symlink(
                "subsystem",
                relative_from_sys_devices(
                    device.path.as_str(),
                    alloc::format!("bus/{bus_name}").as_str(),
                ),
            ),
        )?;

        let bus_devices_dir = ensure_dir(
            &ensure_dir(
                &ensure_dir(&self.sys_root, "bus", 0o040755)?,
                bus_name,
                0o040755,
            )?,
            "devices",
            0o040755,
        )?;
        install_or_replace_child(
            &bus_devices_dir,
            tmpfs::symlink(
                device.name.as_str(),
                alloc::format!("../../../devices/{}", device.path),
            ),
        )?;

        Ok(())
    }

    pub fn register_pci_bus(&self) -> FsResult<()> {
        self.register_sysfs_bus(&SysfsBus::new("pci"))?;

        for device in aether_frame::bus::pci::devices() {
            let sysfs_device = pci_sysfs_device(&device);
            self.register_sysfs_bus_device("pci", &sysfs_device)?;
        }

        Ok(())
    }
}

fn ensure_dir(parent: &NodeRef, name: &str, mode: u32) -> FsResult<NodeRef> {
    if let Some(existing) = parent.lookup(name) {
        if existing.kind() == NodeKind::Directory {
            return Ok(existing);
        }
        return Err(FsError::AlreadyExists);
    }
    parent.create_dir(name.to_string(), mode)
}

fn install_child(parent: &NodeRef, node: NodeRef) -> FsResult<()> {
    parent.insert_child(node.name().to_string(), node)
}

fn install_or_replace_child(parent: &NodeRef, node: NodeRef) -> FsResult<()> {
    if parent.lookup(node.name()).is_some() {
        parent.remove_child(node.name(), node.kind() == NodeKind::Directory)?;
    }
    install_child(parent, node)
}

fn ensure_dir_path(root: &NodeRef, path: &str, mode: u32) -> FsResult<NodeRef> {
    let mut current = root.clone();
    for component in path.split('/').filter(|component| !component.is_empty()) {
        current = ensure_dir(&current, component, mode)?;
    }
    Ok(current)
}

fn install_sysfs_attribute(parent: &NodeRef, attribute: &SysfsAttribute) -> FsResult<()> {
    install_sysfs_file(parent, &attribute.name, attribute.mode, &attribute.source)
}

fn install_sysfs_bin_attribute(parent: &NodeRef, attribute: &SysfsBinAttribute) -> FsResult<()> {
    install_sysfs_file(parent, &attribute.name, attribute.mode, &attribute.source)
}

fn install_sysfs_file(
    parent: &NodeRef,
    name: &str,
    mode: u32,
    source: &SysfsFileSource,
) -> FsResult<()> {
    let node = match source {
        SysfsFileSource::Static(bytes) => tmpfs::file_with_mode(name, bytes.as_slice(), mode),
        SysfsFileSource::Generated(generator) => {
            generated_bytes_file(name, mode, generator.clone())
        }
    };
    install_or_replace_child(parent, node)
}

fn relative_from_sys_devices(path: &str, target: &str) -> String {
    let depth = path
        .split('/')
        .filter(|component| !component.is_empty())
        .count()
        + 1;
    let mut relative = String::new();
    for _ in 0..depth {
        relative.push_str("../");
    }
    relative.push_str(target.trim_start_matches('/'));
    relative
}

fn pci_sysfs_device(device: &PciDeviceInfo) -> SysfsDevice {
    let subsystem_vendor = device.ids.subsystem_vendor_id.unwrap_or(0);
    let subsystem_device = device.ids.subsystem_device_id.unwrap_or(0);

    let mut sysfs_device = SysfsDevice::new(device.name(), device.path.clone())
        .with_attribute(SysfsAttribute::text(
            "class",
            alloc::format!("0x{:06x}\n", device.class.encoded()).into_bytes(),
        ))
        .with_attribute(SysfsAttribute::text(
            "vendor",
            alloc::format!("0x{:04x}\n", device.ids.vendor_id).into_bytes(),
        ))
        .with_attribute(SysfsAttribute::text(
            "device",
            alloc::format!("0x{:04x}\n", device.ids.device_id).into_bytes(),
        ))
        .with_attribute(SysfsAttribute::text(
            "revision",
            alloc::format!("0x{:02x}\n", device.class.revision).into_bytes(),
        ))
        .with_attribute(SysfsAttribute::text(
            "subsystem_vendor",
            alloc::format!("0x{:04x}\n", subsystem_vendor).into_bytes(),
        ))
        .with_attribute(SysfsAttribute::text(
            "subsystem_device",
            alloc::format!("0x{:04x}\n", subsystem_device).into_bytes(),
        ))
        .with_attribute(SysfsAttribute::text(
            "irq",
            alloc::format!("{}\n", device.irq_line.unwrap_or(0)).into_bytes(),
        ))
        .with_attribute(SysfsAttribute::text(
            "modalias",
            pci_modalias(device).into_bytes(),
        ))
        .with_bin_attribute(SysfsBinAttribute::generated_bytes(
            "config",
            0o100444,
            Arc::new({
                let device = device.clone();
                move || device.read_config_bytes()
            }),
        ));

    if device.is_bridge() {
        if let Some(secondary_bus) = device.secondary_bus {
            sysfs_device = sysfs_device.with_attribute(SysfsAttribute::text(
                "secondary_bus_number",
                alloc::format!("{secondary_bus}\n").into_bytes(),
            ));
        }
        if let Some(subordinate_bus) = device.subordinate_bus {
            sysfs_device = sysfs_device.with_attribute(SysfsAttribute::text(
                "subordinate_bus_number",
                alloc::format!("{subordinate_bus}\n").into_bytes(),
            ));
        }
    }

    sysfs_device
}

fn pci_modalias(device: &PciDeviceInfo) -> String {
    alloc::format!(
        "pci:v{:08X}d{:08X}sv{:08X}sd{:08X}bc{:02X}sc{:02X}i{:02X}\n",
        u32::from(device.ids.vendor_id),
        u32::from(device.ids.device_id),
        u32::from(device.ids.subsystem_vendor_id.unwrap_or(0)),
        u32::from(device.ids.subsystem_device_id.unwrap_or(0)),
        device.class.class,
        device.class.subclass,
        device.class.prog_if,
    )
}

fn class_name(class: DeviceClass) -> &'static str {
    match class {
        DeviceClass::Block => "block",
        DeviceClass::Display => "graphics",
        DeviceClass::Drm => "drm",
        DeviceClass::Input => "input",
        DeviceClass::Console => "tty",
        DeviceClass::MessageBuffer => "misc",
        DeviceClass::Misc => "misc",
    }
}

fn render_filesystems(filesystems: &[&str]) -> String {
    let mut buffer = String::new();
    for fstype in filesystems {
        let nodev = matches!(*fstype, "devtmpfs" | "proc" | "rootfs" | "sysfs" | "tmpfs");
        if nodev {
            buffer.push_str("nodev\t");
        } else {
            buffer.push('\t');
        }
        buffer.push_str(fstype);
        buffer.push('\n');
    }
    buffer
}

fn render_uptime() -> String {
    let nanos = time::monotonic_nanos();
    let secs = nanos / 1_000_000_000;
    let centis = (nanos % 1_000_000_000) / 10_000_000;
    alloc::format!("{secs}.{centis:02} 0.00\n")
}

#[allow(clippy::too_many_arguments)]
fn render_uevent_file(
    class: DeviceClass,
    name: &str,
    devpath: &str,
    subsystem: &str,
    major: u16,
    minor: u16,
    nodes: &[DeviceNode],
    extra_fields: &[String],
) -> String {
    let devname = nodes.first().map(|node| node.path.as_str()).unwrap_or(name);
    let devtype = match class {
        DeviceClass::Block => "disk",
        DeviceClass::Drm => "drm_minor",
        DeviceClass::Input => "input",
        DeviceClass::Console => "tty",
        _ => "device",
    };

    let mut content = alloc::format!(
        "DEVPATH={devpath}\nSUBSYSTEM={subsystem}\nMAJOR={major}\nMINOR={minor}\nDEVNAME={devname}\nDEVTYPE={devtype}\n"
    );
    for field in extra_fields {
        content.push_str(field.as_str());
        content.push('\n');
    }
    content
}

#[allow(clippy::too_many_arguments)]
fn render_uevent_message(
    action: &str,
    class: DeviceClass,
    name: &str,
    devpath: &str,
    subsystem: &str,
    major: u16,
    minor: u16,
    nodes: &[DeviceNode],
    seqnum: u64,
    extra_fields: &[String],
) -> Vec<u8> {
    let devname = nodes.first().map(|node| node.path.as_str()).unwrap_or(name);
    let devtype = match class {
        DeviceClass::Block => "disk",
        DeviceClass::Drm => "drm_minor",
        DeviceClass::Input => "input",
        DeviceClass::Console => "tty",
        _ => "device",
    };

    let mut bytes = Vec::new();
    push_uevent_field(&mut bytes, alloc::format!("{action}@{devpath}").as_str());
    push_uevent_field(&mut bytes, alloc::format!("ACTION={action}").as_str());
    push_uevent_field(&mut bytes, alloc::format!("DEVPATH={devpath}").as_str());
    push_uevent_field(&mut bytes, alloc::format!("SUBSYSTEM={subsystem}").as_str());
    push_uevent_field(&mut bytes, alloc::format!("MAJOR={major}").as_str());
    push_uevent_field(&mut bytes, alloc::format!("MINOR={minor}").as_str());
    push_uevent_field(&mut bytes, alloc::format!("DEVNAME={devname}").as_str());
    push_uevent_field(&mut bytes, alloc::format!("DEVTYPE={devtype}").as_str());
    push_uevent_field(&mut bytes, alloc::format!("SEQNUM={seqnum}").as_str());
    for field in extra_fields {
        push_uevent_field(&mut bytes, field.as_str());
    }
    bytes
}

fn push_uevent_field(buffer: &mut Vec<u8>, field: &str) {
    buffer.extend_from_slice(field.as_bytes());
    buffer.push(0);
}

fn field_string(field: &[u8; 65]) -> String {
    let len = field
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(field.len());
    String::from_utf8_lossy(&field[..len]).into_owned()
}

fn c_string_file(value: &str) -> Vec<u8> {
    let mut bytes = value.as_bytes().to_vec();
    bytes.push(b'\n');
    bytes
}

fn render_uevent_seqnum() -> String {
    alloc::format!("{}\n", current_kobject_uevent_seqnum())
}
