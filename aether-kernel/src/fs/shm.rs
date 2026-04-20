extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use aether_frame::libs::spin::SpinLock;
use aether_frame::mm::PAGE_SIZE;
use aether_frame::time;
use aether_vfs::{FileNode, OpenFileDescription, OpenFlags, SharedMemoryFile, SharedOpenFile};

use crate::credentials::Credentials;
use crate::errno::{SysErr, SysResult};
use crate::process::{KernelProcess, ProcessServices, ProcessSyscallContext};
use crate::syscall::KernelSyscallContext;

const IPC_PRIVATE: i32 = 0;
const IPC_CREAT: i32 = 0o1000;
const IPC_EXCL: i32 = 0o2000;

const IPC_RMID: i32 = 0;
const IPC_SET: i32 = 1;
const IPC_STAT: i32 = 2;

const SHM_RDONLY: i32 = 0o10000;
const SHM_RND: i32 = 0o20000;
const SHM_REMAP: i32 = 0o40000;
const SHM_EXEC: i32 = 0o100000;

const MAP_SHARED: u64 = 0x01;
const MAP_FIXED: u64 = 0x10;
const MAP_FIXED_NOREPLACE: u64 = 0x100000;
const PROT_READ: u64 = 0x1;
const PROT_WRITE: u64 = 0x2;
const PROT_EXEC: u64 = 0x4;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct LinuxIpcPerm {
    key: i32,
    uid: u32,
    gid: u32,
    cuid: u32,
    cgid: u32,
    mode: u32,
    seq: u16,
    pad2: u16,
    reserved1: u64,
    reserved2: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct LinuxShmidDs {
    shm_perm: LinuxIpcPerm,
    shm_segsz: usize,
    shm_atime: i64,
    shm_dtime: i64,
    shm_ctime: i64,
    shm_cpid: i32,
    shm_lpid: i32,
    shm_nattch: u64,
    reserved5: u64,
    reserved6: u64,
}

impl LinuxShmidDs {
    fn to_bytes(self) -> [u8; core::mem::size_of::<Self>()] {
        let mut bytes = [0u8; core::mem::size_of::<Self>()];
        bytes[0..4].copy_from_slice(&self.shm_perm.key.to_ne_bytes());
        bytes[4..8].copy_from_slice(&self.shm_perm.uid.to_ne_bytes());
        bytes[8..12].copy_from_slice(&self.shm_perm.gid.to_ne_bytes());
        bytes[12..16].copy_from_slice(&self.shm_perm.cuid.to_ne_bytes());
        bytes[16..20].copy_from_slice(&self.shm_perm.cgid.to_ne_bytes());
        bytes[20..24].copy_from_slice(&self.shm_perm.mode.to_ne_bytes());
        bytes[24..26].copy_from_slice(&self.shm_perm.seq.to_ne_bytes());
        bytes[26..28].copy_from_slice(&self.shm_perm.pad2.to_ne_bytes());
        bytes[28..36].copy_from_slice(&self.shm_perm.reserved1.to_ne_bytes());
        bytes[36..44].copy_from_slice(&self.shm_perm.reserved2.to_ne_bytes());
        bytes[44..52].copy_from_slice(&(self.shm_segsz as u64).to_ne_bytes());
        bytes[52..60].copy_from_slice(&self.shm_atime.to_ne_bytes());
        bytes[60..68].copy_from_slice(&self.shm_dtime.to_ne_bytes());
        bytes[68..76].copy_from_slice(&self.shm_ctime.to_ne_bytes());
        bytes[76..80].copy_from_slice(&self.shm_cpid.to_ne_bytes());
        bytes[80..84].copy_from_slice(&self.shm_lpid.to_ne_bytes());
        bytes[84..92].copy_from_slice(&self.shm_nattch.to_ne_bytes());
        bytes[92..100].copy_from_slice(&self.reserved5.to_ne_bytes());
        bytes[100..108].copy_from_slice(&self.reserved6.to_ne_bytes());
        bytes
    }
}

#[derive(Clone)]
struct SysvShmSegment {
    shmid: i32,
    key: i32,
    size: usize,
    mode: u16,
    uid: u32,
    gid: u32,
    cuid: u32,
    cgid: u32,
    cpid: i32,
    lpid: i32,
    atime: i64,
    dtime: i64,
    ctime: i64,
    nattch: u64,
    marked_destroy: bool,
    file: SharedOpenFile,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct SysvShmAttachment {
    shmid: i32,
    address: u64,
    size: usize,
}

#[derive(Clone, Default)]
struct AddressSpaceAttachments {
    owners: u32,
    attachments: Vec<SysvShmAttachment>,
}

#[derive(Default)]
struct SysvShmRegistry {
    next_shmid: i32,
    segments: BTreeMap<i32, SysvShmSegment>,
    attachments: BTreeMap<usize, AddressSpaceAttachments>,
}

static SYSV_SHM_REGISTRY: SpinLock<SysvShmRegistry> = SpinLock::new(SysvShmRegistry {
    next_shmid: 1,
    segments: BTreeMap::new(),
    attachments: BTreeMap::new(),
});

fn now_seconds() -> i64 {
    time::realtime_seconds()
}

fn page_align_up(size: usize) -> Option<usize> {
    size.checked_add(PAGE_SIZE as usize - 1)
        .map(|size| size & !(PAGE_SIZE as usize - 1))
}

fn build_segment(
    shmid: i32,
    key: i32,
    size: usize,
    shmflg: i32,
    credentials: &Credentials,
    pid: i32,
) -> SysResult<SysvShmSegment> {
    let node = FileNode::new(String::from("sysv-shm"), Arc::new(SharedMemoryFile::new()));
    node.truncate(size).map_err(SysErr::from)?;
    let file = Arc::new(SpinLock::new(OpenFileDescription::new(
        node,
        OpenFlags::from_bits(OpenFlags::READ | OpenFlags::WRITE),
    )));
    let now = now_seconds();
    Ok(SysvShmSegment {
        shmid,
        key,
        size,
        mode: (shmflg & 0o777) as u16,
        uid: credentials.uid,
        gid: credentials.gid,
        cuid: credentials.uid,
        cgid: credentials.gid,
        cpid: pid,
        lpid: 0,
        atime: 0,
        dtime: 0,
        ctime: now,
        nattch: 0,
        marked_destroy: false,
        file,
    })
}

fn reap_segment_if_needed(registry: &mut SysvShmRegistry, shmid: i32) {
    let remove = registry
        .segments
        .get(&shmid)
        .map(|segment| segment.marked_destroy && segment.nattch == 0)
        .unwrap_or(false);
    if remove {
        registry.segments.remove(&shmid);
    }
}

pub(crate) fn register_process(address_space_id: usize) {
    let mut registry = SYSV_SHM_REGISTRY.lock();
    let state = registry.attachments.entry(address_space_id).or_default();
    state.owners = state.owners.saturating_add(1).max(1);
}

pub(crate) fn clone_process(
    parent_address_space_id: usize,
    child_address_space_id: usize,
    shared_vm: bool,
) {
    let mut registry = SYSV_SHM_REGISTRY.lock();
    if shared_vm {
        let state = registry
            .attachments
            .entry(parent_address_space_id)
            .or_default();
        state.owners = state.owners.saturating_add(1).max(1);
        return;
    }

    let parent_attachments = registry
        .attachments
        .get(&parent_address_space_id)
        .map(|state| state.attachments.clone())
        .unwrap_or_default();
    for attachment in &parent_attachments {
        if let Some(segment) = registry.segments.get_mut(&attachment.shmid) {
            segment.nattch = segment.nattch.saturating_add(1);
        }
    }
    registry.attachments.insert(
        child_address_space_id,
        AddressSpaceAttachments {
            owners: 1,
            attachments: parent_attachments,
        },
    );
}

pub(crate) fn replace_process(old_address_space_id: usize, new_address_space_id: usize, pid: i32) {
    register_process(new_address_space_id);
    unregister_process(old_address_space_id, pid);
}

pub(crate) fn unregister_process(address_space_id: usize, pid: i32) {
    let mut registry = SYSV_SHM_REGISTRY.lock();
    let Some(state) = registry.attachments.get_mut(&address_space_id) else {
        return;
    };
    if state.owners > 1 {
        state.owners -= 1;
        return;
    }

    let state = registry
        .attachments
        .remove(&address_space_id)
        .unwrap_or_default();
    let now = now_seconds();
    for attachment in state.attachments {
        if let Some(segment) = registry.segments.get_mut(&attachment.shmid) {
            if segment.nattch > 0 {
                segment.nattch -= 1;
            }
            segment.dtime = now;
            segment.lpid = pid;
        }
        reap_segment_if_needed(&mut registry, attachment.shmid);
    }
}

pub(crate) fn shmget(
    key: i32,
    size: usize,
    shmflg: i32,
    credentials: &Credentials,
    pid: i32,
) -> SysResult<u64> {
    if size == 0 {
        return Err(SysErr::Inval);
    }
    let size = page_align_up(size).ok_or(SysErr::Inval)?;

    let mut registry = SYSV_SHM_REGISTRY.lock();
    if key != IPC_PRIVATE
        && let Some(existing) = registry
            .segments
            .values()
            .find(|segment| segment.key == key && !segment.marked_destroy)
            .cloned()
    {
        if size > existing.size {
            return Err(SysErr::Inval);
        }
        if (shmflg & IPC_EXCL) != 0 {
            return Err(SysErr::Exists);
        }
        return Ok(existing.shmid as u64);
    }

    if key != IPC_PRIVATE && (shmflg & IPC_CREAT) == 0 {
        return Err(SysErr::NoEnt);
    }

    let shmid = registry.next_shmid;
    registry.next_shmid = registry.next_shmid.saturating_add(1);
    let segment = build_segment(shmid, key, size, shmflg, credentials, pid)?;
    registry.segments.insert(shmid, segment);
    Ok(shmid as u64)
}

pub(crate) fn shmat<S: ProcessServices>(
    ctx: &mut ProcessSyscallContext<'_, S>,
    shmid: i32,
    shmaddr: u64,
    shmflg: i32,
) -> SysResult<u64> {
    let (file, size) = {
        let registry = SYSV_SHM_REGISTRY.lock();
        let segment = registry.segments.get(&shmid).ok_or(SysErr::Inval)?;
        (segment.file.clone(), segment.size)
    };

    let requested_address = if shmaddr == 0 {
        0
    } else if (shmflg & SHM_RND) != 0 {
        shmaddr & !(PAGE_SIZE - 1)
    } else if !shmaddr.is_multiple_of(PAGE_SIZE) {
        return Err(SysErr::Inval);
    } else {
        shmaddr
    };

    let mut prot = PROT_READ;
    if (shmflg & SHM_RDONLY) == 0 {
        prot |= PROT_WRITE;
    }
    if (shmflg & SHM_EXEC) != 0 {
        prot |= PROT_EXEC;
    }

    let mut mmap_flags = MAP_SHARED;
    if requested_address != 0 {
        mmap_flags |= if (shmflg & SHM_REMAP) != 0 {
            MAP_FIXED
        } else {
            MAP_FIXED_NOREPLACE
        };
    }

    let mapped = ProcessSyscallContext::<S>::map_file_region(
        ctx.process,
        file,
        requested_address,
        size as u64,
        prot,
        mmap_flags,
        0,
        true,
    )?;

    let mut registry = SYSV_SHM_REGISTRY.lock();
    let segment = registry.segments.get_mut(&shmid).ok_or(SysErr::Inval)?;
    segment.nattch = segment.nattch.saturating_add(1);
    segment.atime = now_seconds();
    segment.lpid = ctx.process.identity.pid as i32;
    let address_space_id = ctx.process.task.address_space.identity();
    let state = registry.attachments.entry(address_space_id).or_default();
    if state.owners == 0 {
        state.owners = 1;
    }
    state.attachments.push(SysvShmAttachment {
        shmid,
        address: mapped,
        size,
    });

    Ok(mapped)
}

pub(crate) fn shmdt(process: &mut KernelProcess, shmaddr: u64) -> SysResult<u64> {
    if shmaddr == 0 {
        return Err(SysErr::Inval);
    }

    let address_space_id = process.task.address_space.identity();
    let attachment = {
        let registry = SYSV_SHM_REGISTRY.lock();
        registry
            .attachments
            .get(&address_space_id)
            .and_then(|state| {
                state
                    .attachments
                    .iter()
                    .find(|attachment| attachment.address == shmaddr)
                    .copied()
            })
            .ok_or(SysErr::Inval)?
    };

    process
        .task
        .address_space
        .munmap(attachment.address, attachment.size as u64)
        .map_err(SysErr::from)?;
    process.remove_mmap_region_range(
        attachment.address,
        attachment.address.saturating_add(attachment.size as u64),
    );

    let mut registry = SYSV_SHM_REGISTRY.lock();
    if let Some(state) = registry.attachments.get_mut(&address_space_id)
        && let Some(index) = state
            .attachments
            .iter()
            .position(|candidate| *candidate == attachment)
    {
        state.attachments.remove(index);
    }
    if let Some(segment) = registry.segments.get_mut(&attachment.shmid) {
        if segment.nattch > 0 {
            segment.nattch -= 1;
        }
        segment.dtime = now_seconds();
        segment.lpid = process.identity.pid as i32;
    }
    reap_segment_if_needed(&mut registry, attachment.shmid);

    Ok(0)
}

pub(crate) fn shmctl<S: ProcessServices>(
    ctx: &mut ProcessSyscallContext<'_, S>,
    shmid: i32,
    cmd: i32,
    buf: u64,
) -> SysResult<u64> {
    let mut registry = SYSV_SHM_REGISTRY.lock();
    let now = now_seconds();
    match cmd {
        IPC_RMID => {
            let segment = registry.segments.get_mut(&shmid).ok_or(SysErr::Inval)?;
            segment.marked_destroy = true;
            segment.ctime = now;
            reap_segment_if_needed(&mut registry, shmid);
            Ok(0)
        }
        IPC_STAT => {
            if buf == 0 {
                return Err(SysErr::Fault);
            }
            let segment = registry.segments.get(&shmid).ok_or(SysErr::Inval)?;
            let stat = LinuxShmidDs {
                shm_perm: LinuxIpcPerm {
                    key: segment.key,
                    uid: segment.uid,
                    gid: segment.gid,
                    cuid: segment.cuid,
                    cgid: segment.cgid,
                    mode: segment.mode as u32,
                    seq: 0,
                    pad2: 0,
                    reserved1: 0,
                    reserved2: 0,
                },
                shm_segsz: segment.size,
                shm_atime: segment.atime,
                shm_dtime: segment.dtime,
                shm_ctime: segment.ctime,
                shm_cpid: segment.cpid,
                shm_lpid: segment.lpid,
                shm_nattch: segment.nattch,
                reserved5: 0,
                reserved6: 0,
            };
            drop(registry);
            ctx.write_user_buffer(buf, &stat.to_bytes())?;
            Ok(0)
        }
        IPC_SET => Err(SysErr::NoSys),
        _ => Err(SysErr::NoSys),
    }
}
