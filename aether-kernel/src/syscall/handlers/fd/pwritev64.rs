use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext, read_iovec_array};
use crate::syscall::{KernelSyscallContext, SyscallDisposition};

fn join_offset(low: u64, high: u64) -> u64 {
    low | (high << 32)
}

crate::declare_syscall!(
    pub struct Pwritev64Syscall => nr::PWRITEV64, "pwritev64", |ctx, args| {
        SyscallDisposition::Return(ctx.pwritev64(
            args.get(0),
            args.get(1),
            args.get(2) as usize,
            join_offset(args.get(3), args.get(4)),
        ))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_pwritev64(
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
            let bytes = self.read_user_buffer(segment.base, segment.len)?;
            let written = node.write(position, &bytes).map_err(SysErr::from)?;
            total = total.saturating_add(written);
            position = position.saturating_add(written);
            if written < bytes.len() {
                break;
            }
        }

        Ok(total as u64)
    }
}
