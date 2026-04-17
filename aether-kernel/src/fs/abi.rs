use aether_vfs::{DirectoryEntry, NodeKind, NodeRef, NodeTimestamp};
use alloc::vec::Vec;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct LinuxStat {
    pub st_dev: u64,
    pub st_ino: u64,
    pub st_nlink: u64,
    pub st_mode: u32,
    pub st_uid: u32,
    pub st_gid: u32,
    pub __pad0: u32,
    pub st_rdev: u64,
    pub st_size: i64,
    pub st_blksize: i64,
    pub st_blocks: i64,
    pub st_atime: i64,
    pub st_atime_nsec: i64,
    pub st_mtime: i64,
    pub st_mtime_nsec: i64,
    pub st_ctime: i64,
    pub st_ctime_nsec: i64,
    pub __unused: [i64; 3],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct LinuxUtsName {
    pub sysname: [u8; 65],
    pub nodename: [u8; 65],
    pub release: [u8; 65],
    pub version: [u8; 65],
    pub machine: [u8; 65],
    pub domainname: [u8; 65],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinuxStatFs {
    pub f_type: u64,
    pub f_bsize: u64,
    pub f_blocks: u64,
    pub f_bfree: u64,
    pub f_bavail: u64,
    pub f_files: u64,
    pub f_ffree: u64,
    pub f_fsid: [i32; 2],
    pub f_namelen: u64,
    pub f_frsize: u64,
    pub f_flags: u64,
    pub f_spare: [u64; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct LinuxStatxTimestamp {
    pub tv_sec: i64,
    pub tv_nsec: u32,
    pub __reserved: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct LinuxStatx {
    pub stx_mask: u32,
    pub stx_blksize: u32,
    pub stx_attributes: u64,
    pub stx_nlink: u32,
    pub stx_uid: u32,
    pub stx_gid: u32,
    pub stx_mode: u16,
    pub __spare0: u16,
    pub stx_ino: u64,
    pub stx_size: u64,
    pub stx_blocks: u64,
    pub stx_attributes_mask: u64,
    pub stx_atime: LinuxStatxTimestamp,
    pub stx_btime: LinuxStatxTimestamp,
    pub stx_ctime: LinuxStatxTimestamp,
    pub stx_mtime: LinuxStatxTimestamp,
    pub stx_rdev_major: u32,
    pub stx_rdev_minor: u32,
    pub stx_dev_major: u32,
    pub stx_dev_minor: u32,
    pub stx_mnt_id: u64,
    pub stx_dio_mem_align: u32,
    pub stx_dio_offset_align: u32,
    pub stx_subvol: u64,
    pub stx_atomic_write_unit_min: u32,
    pub stx_atomic_write_unit_max: u32,
    pub stx_atomic_write_segments_max: u32,
    pub stx_atomic_write_unit_max_opt: u32,
    pub __spare2: u32,
    pub __spare3: [u64; 12],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileSystemIdentity {
    pub device_id: u64,
    pub statfs: LinuxStatFs,
}

impl FileSystemIdentity {
    pub const fn new(device_id: u64, statfs: LinuxStatFs) -> Self {
        Self { device_id, statfs }
    }
}

impl LinuxStatFs {
    pub const fn new(magic: u64, block_size: u64, name_len: u64) -> Self {
        Self {
            f_type: magic,
            f_bsize: block_size,
            f_blocks: 0,
            f_bfree: 0,
            f_bavail: 0,
            f_files: 0,
            f_ffree: 0,
            f_fsid: [0; 2],
            f_namelen: name_len,
            f_frsize: block_size,
            f_flags: 0,
            f_spare: [0; 4],
        }
    }

    pub const fn with_device_id(mut self, device_id: u64) -> Self {
        self.f_fsid = [device_id as i32, (device_id >> 32) as i32];
        self
    }
}

impl LinuxUtsName {
    pub fn linux_x86_64() -> Self {
        let mut uts = Self {
            sysname: [0; 65],
            nodename: [0; 65],
            release: [0; 65],
            version: [0; 65],
            machine: [0; 65],
            domainname: [0; 65],
        };
        write_field(&mut uts.sysname, b"Linux");
        write_field(&mut uts.nodename, b"aether");
        write_field(&mut uts.release, b"6.12.0");
        write_field(&mut uts.version, b"#1 Aether");
        write_field(&mut uts.machine, b"x86_64");
        write_field(&mut uts.domainname, b"localdomain");
        uts
    }
}

pub const STATX_TYPE: u32 = 0x0000_0001;
pub const STATX_MODE: u32 = 0x0000_0002;
pub const STATX_NLINK: u32 = 0x0000_0004;
pub const STATX_UID: u32 = 0x0000_0008;
pub const STATX_GID: u32 = 0x0000_0010;
pub const STATX_ATIME: u32 = 0x0000_0020;
pub const STATX_MTIME: u32 = 0x0000_0040;
pub const STATX_CTIME: u32 = 0x0000_0080;
pub const STATX_INO: u32 = 0x0000_0100;
pub const STATX_SIZE: u32 = 0x0000_0200;
pub const STATX_BLOCKS: u32 = 0x0000_0400;
pub const STATX_BASIC_STATS: u32 = STATX_TYPE
    | STATX_MODE
    | STATX_NLINK
    | STATX_UID
    | STATX_GID
    | STATX_ATIME
    | STATX_MTIME
    | STATX_CTIME
    | STATX_INO
    | STATX_SIZE
    | STATX_BLOCKS;
pub const STATX_BTIME: u32 = 0x0000_0800;
pub const STATX_RESERVED: u32 = 0x8000_0000;

pub fn make_stat(node: &NodeRef) -> LinuxStat {
    let metadata = node.metadata();
    let st_rdev = linux_makedev(
        u64::from(metadata.rdev_major),
        u64::from(metadata.rdev_minor),
    );

    LinuxStat {
        st_dev: metadata.device_id,
        st_ino: metadata.inode,
        st_nlink: u64::from(metadata.nlink),
        st_mode: metadata.mode,
        st_uid: metadata.uid,
        st_gid: metadata.gid,
        st_rdev,
        st_size: metadata.size.min(i64::MAX as u64) as i64,
        st_blksize: i64::from(metadata.block_size),
        st_blocks: metadata.blocks.min(i64::MAX as u64) as i64,
        st_atime: metadata.atime.secs,
        st_atime_nsec: i64::from(metadata.atime.nanos),
        st_mtime: metadata.mtime.secs,
        st_mtime_nsec: i64::from(metadata.mtime.nanos),
        st_ctime: metadata.ctime.secs,
        st_ctime_nsec: i64::from(metadata.ctime.nanos),
        ..LinuxStat::default()
    }
}

pub fn make_statx(node: &NodeRef, mask: u32) -> LinuxStatx {
    let metadata = node.metadata();
    let supported = STATX_BASIC_STATS | STATX_BTIME;
    let result_mask = if mask == 0 {
        supported
    } else {
        supported & mask
    };
    let (dev_major, dev_minor) = linux_dev_parts(metadata.device_id);

    LinuxStatx {
        stx_mask: result_mask,
        stx_blksize: metadata.block_size,
        stx_nlink: metadata.nlink,
        stx_uid: metadata.uid,
        stx_gid: metadata.gid,
        stx_mode: metadata.mode as u16,
        stx_ino: metadata.inode,
        stx_size: metadata.size,
        stx_blocks: metadata.blocks,
        stx_atime: statx_timestamp(metadata.atime),
        stx_btime: statx_timestamp(metadata.btime),
        stx_ctime: statx_timestamp(metadata.ctime),
        stx_mtime: statx_timestamp(metadata.mtime),
        stx_rdev_major: metadata.rdev_major,
        stx_rdev_minor: metadata.rdev_minor,
        stx_dev_major: dev_major,
        stx_dev_minor: dev_minor,
        ..LinuxStatx::default()
    }
}

pub fn serialize_stat(stat: &LinuxStat) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(144);
    bytes.extend_from_slice(&stat.st_dev.to_ne_bytes());
    bytes.extend_from_slice(&stat.st_ino.to_ne_bytes());
    bytes.extend_from_slice(&stat.st_nlink.to_ne_bytes());
    bytes.extend_from_slice(&stat.st_mode.to_ne_bytes());
    bytes.extend_from_slice(&stat.st_uid.to_ne_bytes());
    bytes.extend_from_slice(&stat.st_gid.to_ne_bytes());
    bytes.extend_from_slice(&stat.__pad0.to_ne_bytes());
    bytes.extend_from_slice(&stat.st_rdev.to_ne_bytes());
    bytes.extend_from_slice(&stat.st_size.to_ne_bytes());
    bytes.extend_from_slice(&stat.st_blksize.to_ne_bytes());
    bytes.extend_from_slice(&stat.st_blocks.to_ne_bytes());
    bytes.extend_from_slice(&stat.st_atime.to_ne_bytes());
    bytes.extend_from_slice(&stat.st_atime_nsec.to_ne_bytes());
    bytes.extend_from_slice(&stat.st_mtime.to_ne_bytes());
    bytes.extend_from_slice(&stat.st_mtime_nsec.to_ne_bytes());
    bytes.extend_from_slice(&stat.st_ctime.to_ne_bytes());
    bytes.extend_from_slice(&stat.st_ctime_nsec.to_ne_bytes());
    for value in stat.__unused {
        bytes.extend_from_slice(&value.to_ne_bytes());
    }
    bytes
}

pub fn serialize_statx(statx: &LinuxStatx) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(core::mem::size_of::<LinuxStatx>());
    bytes.extend_from_slice(&statx.stx_mask.to_ne_bytes());
    bytes.extend_from_slice(&statx.stx_blksize.to_ne_bytes());
    bytes.extend_from_slice(&statx.stx_attributes.to_ne_bytes());
    bytes.extend_from_slice(&statx.stx_nlink.to_ne_bytes());
    bytes.extend_from_slice(&statx.stx_uid.to_ne_bytes());
    bytes.extend_from_slice(&statx.stx_gid.to_ne_bytes());
    bytes.extend_from_slice(&statx.stx_mode.to_ne_bytes());
    bytes.extend_from_slice(&statx.__spare0.to_ne_bytes());
    bytes.extend_from_slice(&statx.stx_ino.to_ne_bytes());
    bytes.extend_from_slice(&statx.stx_size.to_ne_bytes());
    bytes.extend_from_slice(&statx.stx_blocks.to_ne_bytes());
    bytes.extend_from_slice(&statx.stx_attributes_mask.to_ne_bytes());
    serialize_statx_timestamp(&mut bytes, &statx.stx_atime);
    serialize_statx_timestamp(&mut bytes, &statx.stx_btime);
    serialize_statx_timestamp(&mut bytes, &statx.stx_ctime);
    serialize_statx_timestamp(&mut bytes, &statx.stx_mtime);
    bytes.extend_from_slice(&statx.stx_rdev_major.to_ne_bytes());
    bytes.extend_from_slice(&statx.stx_rdev_minor.to_ne_bytes());
    bytes.extend_from_slice(&statx.stx_dev_major.to_ne_bytes());
    bytes.extend_from_slice(&statx.stx_dev_minor.to_ne_bytes());
    bytes.extend_from_slice(&statx.stx_mnt_id.to_ne_bytes());
    bytes.extend_from_slice(&statx.stx_dio_mem_align.to_ne_bytes());
    bytes.extend_from_slice(&statx.stx_dio_offset_align.to_ne_bytes());
    bytes.extend_from_slice(&statx.stx_subvol.to_ne_bytes());
    bytes.extend_from_slice(&statx.stx_atomic_write_unit_min.to_ne_bytes());
    bytes.extend_from_slice(&statx.stx_atomic_write_unit_max.to_ne_bytes());
    bytes.extend_from_slice(&statx.stx_atomic_write_segments_max.to_ne_bytes());
    bytes.extend_from_slice(&statx.stx_atomic_write_unit_max_opt.to_ne_bytes());
    bytes.extend_from_slice(&statx.__spare2.to_ne_bytes());
    for value in statx.__spare3 {
        bytes.extend_from_slice(&value.to_ne_bytes());
    }
    bytes
}

pub fn serialize_statfs(statfs: &LinuxStatFs) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(120);
    bytes.extend_from_slice(&statfs.f_type.to_ne_bytes());
    bytes.extend_from_slice(&statfs.f_bsize.to_ne_bytes());
    bytes.extend_from_slice(&statfs.f_blocks.to_ne_bytes());
    bytes.extend_from_slice(&statfs.f_bfree.to_ne_bytes());
    bytes.extend_from_slice(&statfs.f_bavail.to_ne_bytes());
    bytes.extend_from_slice(&statfs.f_files.to_ne_bytes());
    bytes.extend_from_slice(&statfs.f_ffree.to_ne_bytes());
    bytes.extend_from_slice(&statfs.f_fsid[0].to_ne_bytes());
    bytes.extend_from_slice(&statfs.f_fsid[1].to_ne_bytes());
    bytes.extend_from_slice(&statfs.f_namelen.to_ne_bytes());
    bytes.extend_from_slice(&statfs.f_frsize.to_ne_bytes());
    bytes.extend_from_slice(&statfs.f_flags.to_ne_bytes());
    for value in statfs.f_spare {
        bytes.extend_from_slice(&value.to_ne_bytes());
    }
    bytes
}

pub fn serialize_utsname(uts: &LinuxUtsName) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(65 * 6);
    bytes.extend_from_slice(&uts.sysname);
    bytes.extend_from_slice(&uts.nodename);
    bytes.extend_from_slice(&uts.release);
    bytes.extend_from_slice(&uts.version);
    bytes.extend_from_slice(&uts.machine);
    bytes.extend_from_slice(&uts.domainname);
    bytes
}

pub fn serialize_dirents64(entries: &[DirectoryEntry], start: usize, capacity: usize) -> Vec<u8> {
    let mut buffer = Vec::new();

    for (index, entry) in entries.iter().enumerate().skip(start) {
        let name = entry.name.as_bytes();
        let reclen = align_up(8 + 8 + 2 + 1 + name.len() + 1, 8);
        if buffer.len() + reclen > capacity {
            break;
        }

        buffer.extend_from_slice(&(index as u64 + 1).to_ne_bytes());
        buffer.extend_from_slice(&((index + 1) as i64).to_ne_bytes());
        buffer.extend_from_slice(&(reclen as u16).to_ne_bytes());
        buffer.push(dirent_type(entry.kind));
        buffer.extend_from_slice(name);
        buffer.push(0);
        while buffer.len() % 8 != 0 {
            buffer.push(0);
        }
    }

    buffer
}

fn dirent_type(kind: NodeKind) -> u8 {
    match kind {
        NodeKind::Directory => 4,
        NodeKind::File => 8,
        NodeKind::Socket => 12,
        NodeKind::Symlink => 10,
        NodeKind::BlockDevice => 6,
        NodeKind::CharDevice => 2,
        NodeKind::Fifo => 1,
    }
}

fn linux_makedev(major: u64, minor: u64) -> u64 {
    (minor & 0xff) | ((major & 0xfff) << 8) | ((minor & !0xff) << 12) | ((major & !0xfff) << 32)
}

fn linux_dev_parts(dev: u64) -> (u32, u32) {
    let major = (((dev >> 8) & 0xfff) | ((dev >> 32) & !0xfff)).min(u64::from(u32::MAX)) as u32;
    let minor = ((dev & 0xff) | ((dev >> 12) & !0xff)).min(u64::from(u32::MAX)) as u32;
    (major, minor)
}

fn statx_timestamp(timestamp: NodeTimestamp) -> LinuxStatxTimestamp {
    LinuxStatxTimestamp {
        tv_sec: timestamp.secs,
        tv_nsec: timestamp.nanos,
        __reserved: 0,
    }
}

fn serialize_statx_timestamp(bytes: &mut Vec<u8>, timestamp: &LinuxStatxTimestamp) {
    bytes.extend_from_slice(&timestamp.tv_sec.to_ne_bytes());
    bytes.extend_from_slice(&timestamp.tv_nsec.to_ne_bytes());
    bytes.extend_from_slice(&timestamp.__reserved.to_ne_bytes());
}

fn align_up(value: usize, align: usize) -> usize {
    (value + align - 1) & !(align - 1)
}

fn write_field(field: &mut [u8; 65], value: &[u8]) {
    let len = core::cmp::min(field.len().saturating_sub(1), value.len());
    field[..len].copy_from_slice(&value[..len]);
}
