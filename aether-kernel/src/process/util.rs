use alloc::vec::Vec;

use super::ChildEventKind;
use crate::errno::{SysErr, SysResult};
use crate::fs::{FileSystemIdentity, LinuxStatFs};
use crate::signal::SigSet;

pub(super) fn child_event_matches_options(kind: ChildEventKind, options: u64) -> bool {
    const WUNTRACED: u64 = 2;
    const WCONTINUED: u64 = 8;

    match kind {
        ChildEventKind::Exited(_) => true,
        ChildEventKind::Stopped(_) => (options & WUNTRACED) != 0,
        ChildEventKind::Continued => (options & WCONTINUED) != 0,
    }
}

pub(crate) fn wait_status(kind: ChildEventKind) -> i32 {
    match kind {
        ChildEventKind::Exited(status) => {
            if status >= 128 {
                // TODO: set the Linux core-dump bit once the kernel tracks that termination mode.
                status - 128
            } else {
                (status & 0xff) << 8
            }
        }
        ChildEventKind::Stopped(signal) => ((signal as i32) << 8) | 0x7f,
        ChildEventKind::Continued => 0xffff,
    }
}

pub(crate) fn anonymous_filesystem_identity() -> FileSystemIdentity {
    FileSystemIdentity::new(0, LinuxStatFs::new(0, 4096, 255))
}

pub(crate) fn decode_sigset(bytes: &[u8]) -> SigSet {
    let mut raw = [0u8; 8];
    let len = core::cmp::min(raw.len(), bytes.len());
    raw[..len].copy_from_slice(&bytes[..len]);
    u64::from_ne_bytes(raw)
}

#[derive(Clone, Copy)]
pub(crate) struct UserIoVec {
    pub base: u64,
    pub len: usize,
}

pub(crate) fn read_iovec_array(
    address_space: &aether_process::UserAddressSpace,
    address: u64,
    count: usize,
) -> SysResult<Vec<UserIoVec>> {
    if count == 0 {
        return Ok(Vec::new());
    }

    let raw = address_space
        .read_user_exact(address, count.saturating_mul(16))
        .map_err(|_| SysErr::Fault)?;
    let mut iovecs = Vec::with_capacity(count);
    for chunk in raw.chunks_exact(16) {
        let mut base = [0u8; 8];
        let mut len = [0u8; 8];
        base.copy_from_slice(&chunk[..8]);
        len.copy_from_slice(&chunk[8..16]);
        iovecs.push(UserIoVec {
            base: u64::from_ne_bytes(base),
            len: u64::from_ne_bytes(len).min(usize::MAX as u64) as usize,
        });
    }
    Ok(iovecs)
}
