use crate::arch::syscall::nr;
use crate::declare_syscall;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::KernelSyscallContext;
use crate::syscall::SyscallDisposition;

declare_syscall! {
    pub struct PipeSyscall => nr::PIPE, "pipe", |ctx, args| {
        SyscallDisposition::Return(ctx.pipe(args.get(0), 0))
    }
}

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_pipe(&mut self, pipefd: u64, flags: u64) -> SysResult<u64> {
        const O_CLOEXEC: u64 = 0o2000000;
        const O_NONBLOCK: u64 = 0o4000;

        if (flags & !(O_CLOEXEC | O_NONBLOCK)) != 0 {
            return Err(SysErr::Inval);
        }

        let (read_end, write_end) = aether_fs::anonymous_pipe();
        let filesystem = crate::process::anonymous_filesystem_identity();
        let cloexec = (flags & O_CLOEXEC) != 0;
        let mut read_flags = aether_vfs::OpenFlags::from_bits(aether_vfs::OpenFlags::READ);
        let mut write_flags = aether_vfs::OpenFlags::from_bits(aether_vfs::OpenFlags::WRITE);
        if (flags & O_NONBLOCK) != 0 {
            read_flags = aether_vfs::OpenFlags::from_bits(
                read_flags.bits() | aether_vfs::OpenFlags::NONBLOCK,
            );
            write_flags = aether_vfs::OpenFlags::from_bits(
                write_flags.bits() | aether_vfs::OpenFlags::NONBLOCK,
            );
        }

        let read_fd = self
            .process
            .files
            .insert_node(read_end, read_flags, filesystem, None, cloexec);
        let write_fd =
            self.process
                .files
                .insert_node(write_end, write_flags, filesystem, None, cloexec);

        let mut bytes = [0u8; 8];
        bytes[..4].copy_from_slice(&(read_fd as i32).to_ne_bytes());
        bytes[4..].copy_from_slice(&(write_fd as i32).to_ne_bytes());
        self.write_user_buffer(pipefd, &bytes)?;
        Ok(0)
    }
}
