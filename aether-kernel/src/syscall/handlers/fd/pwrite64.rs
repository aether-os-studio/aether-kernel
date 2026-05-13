use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::ProcessSyscallContext;
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct Pwrite64Syscall => nr::PWRITE64, "pwrite64", |ctx, args| {
        SyscallDisposition::Return(ctx.pwrite64(
            args.get(0),
            args.get(1),
            args.get(2) as usize,
            args.get(3),
        ))
    }
);

impl ProcessSyscallContext<'_> {
    pub(crate) fn pwrite64(
        &mut self,
        fd: u64,
        address: u64,
        len: usize,
        offset: u64,
    ) -> SysResult<u64> {
        let descriptor = self.process.files.get(fd as u32).ok_or(SysErr::BadFd)?;
        let node = descriptor.file.lock().node();
        let bytes = self.read_user_buffer(address, len)?;
        let written = node.write(offset as usize, &bytes).map_err(SysErr::from)?;
        Ok(written as u64)
    }
}
