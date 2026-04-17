use alloc::vec;

use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext, read_iovec_array};
use crate::syscall::KernelSyscallContext;
use crate::syscall::SyscallDisposition;

fn join_offset(low: u64, high: u64) -> u64 {
    low | (high << 32)
}

crate::declare_syscall!(
    pub struct Preadv64Syscall => nr::PREADV64, "preadv64", |ctx, args| {
        SyscallDisposition::Return(ctx.preadv64(
            args.get(0),
            args.get(1),
            args.get(2) as usize,
            join_offset(args.get(3), args.get(4)),
        ))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_preadv64(
        &mut self,
        fd: u64,
        iov: u64,
        iovcnt: usize,
        offset: u64,
    ) -> SysResult<u64> {
        const IOV_MAX: usize = 1024;

        let descriptor = self.process.files.get(fd as u32).ok_or(SysErr::BadFd)?;
        let node = descriptor.file.lock().node();
        let segments =
            read_iovec_array(&self.process.task.address_space, iov, iovcnt.min(IOV_MAX))?;
        let mut total = 0usize;
        let mut position = offset as usize;

        for segment in segments {
            if segment.len == 0 {
                continue;
            }
            let mut buffer = vec![0u8; segment.len];
            let read = node.read(position, &mut buffer).map_err(SysErr::from)?;
            if read == 0 {
                break;
            }
            self.write_user_buffer(segment.base, &buffer[..read])?;
            total = total.saturating_add(read);
            position = position.saturating_add(read);
            if read < segment.len {
                break;
            }
        }

        Ok(total as u64)
    }
}
