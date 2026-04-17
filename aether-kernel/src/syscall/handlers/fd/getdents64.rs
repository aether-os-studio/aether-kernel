use crate::arch::syscall::nr;
use aether_vfs::NodeKind;

use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::KernelSyscallContext;
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct Getdents64Syscall => nr::GETDENTS64, "getdents64", |ctx, args| {
        SyscallDisposition::Return(ctx.getdents64(args.get(0), args.get(1), args.get(2) as usize))
    }
);

fn count_dirents(buffer: &[u8]) -> usize {
    let mut count = 0;
    let mut offset = 0usize;
    while offset + 19 <= buffer.len() {
        let reclen = u16::from_ne_bytes([buffer[offset + 16], buffer[offset + 17]]) as usize;
        if reclen == 0 || offset + reclen > buffer.len() {
            break;
        }
        count += 1;
        offset += reclen;
    }
    count
}

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_getdents64(
        &mut self,
        fd: u64,
        address: u64,
        len: usize,
    ) -> SysResult<u64> {
        let descriptor = self.process.files.get(fd as u32).ok_or(SysErr::BadFd)?;
        let mut file = descriptor.file.lock();
        let node = file.node();
        if node.kind() != NodeKind::Directory {
            return Err(SysErr::NotDir);
        }

        let entries = node.entries();
        let offset = file.position();
        let bytes = crate::fs::serialize_dirents64(&entries, offset, len);
        file.set_position(core::cmp::min(
            entries.len(),
            offset + count_dirents(&bytes),
        ));
        drop(file);
        self.write_user_buffer(address, &bytes)?;
        Ok(bytes.len() as u64)
    }
}
