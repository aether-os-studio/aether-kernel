use aether_frame::time;
use aether_vfs::NodeTimestamp;

use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::ProcessSyscallContext;
use crate::syscall::SyscallDisposition;
use crate::syscall::abi::{arg_i64_from_i32, read_path_allow_empty};

const AT_SYMLINK_NOFOLLOW: u64 = 0x100;
const AT_EMPTY_PATH: u64 = 0x1000;
const UTIME_NOW: i64 = 0x3fffffff;
const UTIME_OMIT: i64 = 0x3ffffffe;

crate::declare_syscall!(
    pub struct UtimensatSyscall => nr::UTIMENSAT, "utimensat", |ctx, args| {
        let flags = args.get(3);
        let Ok(path) = read_path_allow_empty(ctx, args.get(1), flags, 4096) else {
            return SyscallDisposition::Return(Err(crate::errno::SysErr::Fault));
        };
        SyscallDisposition::Return(ctx.utimensat(
            arg_i64_from_i32(args.get(0)),
            &path,
            args.get(2),
            flags,
        ))
    }
);

#[derive(Clone, Copy)]
enum UtimeValue {
    Set(NodeTimestamp),
    Now,
    Omit,
}

impl ProcessSyscallContext<'_> {
    pub(crate) fn utimensat(
        &mut self,
        dirfd: i64,
        path: &str,
        times: u64,
        flags: u64,
    ) -> SysResult<u64> {
        if (flags & !(AT_SYMLINK_NOFOLLOW | AT_EMPTY_PATH)) != 0 {
            return Err(SysErr::Inval);
        }

        let [atime, mtime] = if times == 0 {
            [UtimeValue::Now, UtimeValue::Now]
        } else {
            self.read_utimensat_times(times)?
        };

        let node = if (flags & AT_EMPTY_PATH) != 0 && path.is_empty() {
            if dirfd == crate::syscall::abi::AT_FDCWD {
                self.process.fs.cwd_node()
            } else {
                let descriptor = self.process.files.get(dirfd as u32).ok_or(SysErr::BadFd)?;
                descriptor.file.lock().node()
            }
        } else {
            let fs_view = self.fs_view_for_dirfd(dirfd, path)?;
            let (node, _) = self.services.lookup_node_with_identity(
                &fs_view,
                path,
                (flags & AT_SYMLINK_NOFOLLOW) == 0,
            )?;
            node
        };

        let now = realtime_timestamp();
        let new_atime = match atime {
            UtimeValue::Set(timestamp) => Some(timestamp),
            UtimeValue::Now => Some(now),
            UtimeValue::Omit => None,
        };
        let new_mtime = match mtime {
            UtimeValue::Set(timestamp) => Some(timestamp),
            UtimeValue::Now => Some(now),
            UtimeValue::Omit => None,
        };

        node.set_times(new_atime, new_mtime, now)
            .map_err(SysErr::from)?;
        Ok(0)
    }

    fn read_utimensat_times(&self, address: u64) -> SysResult<[UtimeValue; 2]> {
        let bytes = self.read_user_exact_buffer(address, 32)?;
        Ok([
            decode_utimensat_time(&bytes[0..16])?,
            decode_utimensat_time(&bytes[16..32])?,
        ])
    }
}

fn decode_utimensat_time(bytes: &[u8]) -> SysResult<UtimeValue> {
    let secs = i64::from_ne_bytes(bytes[0..8].try_into().map_err(|_| SysErr::Fault)?);
    let nanos = i64::from_ne_bytes(bytes[8..16].try_into().map_err(|_| SysErr::Fault)?);
    match nanos {
        UTIME_NOW => Ok(UtimeValue::Now),
        UTIME_OMIT => Ok(UtimeValue::Omit),
        0..=999_999_999 => Ok(UtimeValue::Set(NodeTimestamp {
            secs,
            nanos: nanos as u32,
        })),
        _ => Err(SysErr::Inval),
    }
}

fn realtime_timestamp() -> NodeTimestamp {
    let (secs, nanos) = time::realtime_nanos();
    NodeTimestamp { secs, nanos }
}
