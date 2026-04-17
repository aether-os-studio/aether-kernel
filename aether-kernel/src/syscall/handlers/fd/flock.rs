use aether_vfs::{FlockOperation, PollEvents};

use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

const LOCK_SH: u64 = 1;
const LOCK_EX: u64 = 2;
const LOCK_NB: u64 = 4;
const LOCK_UN: u64 = 8;
const LOCK_OPERATION_MASK: u64 = LOCK_SH | LOCK_EX | LOCK_UN;
const LOCK_VALID_MASK: u64 = LOCK_OPERATION_MASK | LOCK_NB;

crate::declare_syscall!(pub struct FlockSyscall => nr::FLOCK, "flock", |ctx, args| {
    ctx.flock_blocking(args.get(0), args.get(1))
});

fn parse_flock_operation(operation: u64) -> SysResult<(FlockOperation, bool)> {
    if (operation & !LOCK_VALID_MASK) != 0 {
        return Err(SysErr::Inval);
    }

    let nonblock = (operation & LOCK_NB) != 0;
    let flock_operation = match operation & LOCK_OPERATION_MASK {
        LOCK_SH => FlockOperation::Shared,
        LOCK_EX => FlockOperation::Exclusive,
        LOCK_UN => FlockOperation::Unlock,
        _ => return Err(SysErr::Inval),
    };
    Ok((flock_operation, nonblock))
}

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_flock(&mut self, fd: u64, operation: u64) -> SysResult<u64> {
        let (operation, _) = parse_flock_operation(operation)?;
        let descriptor = self.process.files.get(fd as u32).ok_or(SysErr::BadFd)?;
        descriptor
            .file
            .lock()
            .flock(operation)
            .map_err(SysErr::from)?;
        Ok(0)
    }

    pub(crate) fn syscall_flock_blocking(&mut self, fd: u64, operation: u64) -> SyscallDisposition {
        let (flock_operation, nonblock) = match parse_flock_operation(operation) {
            Ok(parsed) => parsed,
            Err(error) => return SyscallDisposition::err(error),
        };

        if nonblock || matches!(flock_operation, FlockOperation::Unlock) {
            return SyscallDisposition::Return(self.syscall_flock(fd, operation));
        }

        self.restartable_blocking_syscall(
            |ctx| ctx.syscall_flock(fd, operation),
            |ctx| ctx.block_file(fd as u32, PollEvents::LOCK),
        )
    }
}
