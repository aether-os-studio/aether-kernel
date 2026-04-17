use crate::arch::syscall::nr;
use aether_vfs::NodeKind;

use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct LseekSyscall => nr::LSEEK, "lseek", |ctx, args| {
        SyscallDisposition::Return(ctx.lseek(args.get(0), args.get(1) as i64, args.get(2)))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_lseek(&mut self, fd: u64, offset: i64, whence: u64) -> SysResult<u64> {
        const SEEK_SET: u64 = 0;
        const SEEK_CUR: u64 = 1;
        const SEEK_END: u64 = 2;

        let descriptor = self.process.files.get(fd as u32).ok_or(SysErr::BadFd)?;
        let mut file = descriptor.file.lock();
        match file.node().kind() {
            NodeKind::Fifo | NodeKind::Socket => return Err(SysErr::SPipe),
            NodeKind::File
            | NodeKind::Directory
            | NodeKind::Symlink
            | NodeKind::BlockDevice
            | NodeKind::CharDevice => {}
        }

        let base = match whence {
            SEEK_SET => 0i128,
            SEEK_CUR => file.position() as i128,
            SEEK_END => file.node().size() as i128,
            _ => return Err(SysErr::Inval),
        };
        let next = base.saturating_add(offset as i128);
        if next < 0 || next > usize::MAX as i128 {
            return Err(SysErr::Inval);
        }
        file.set_position(next as usize);
        Ok(next as u64)
    }
}
