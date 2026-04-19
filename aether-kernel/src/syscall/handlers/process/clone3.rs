use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{CloneParams, LinuxCloneArgs, ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct Clone3Syscall => nr::CLONE3, "clone3", |ctx, args| {
        ctx.clone3_blocking(args.get(0), args.get(1) as usize)
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_clone3(&mut self, args: u64, size: usize) -> SysResult<u64> {
        let disposition = self.syscall_clone3_blocking(args, size);
        match disposition {
            SyscallDisposition::Return(result) => result,
            _ => Err(SysErr::Again),
        }
    }

    pub(crate) fn syscall_clone3_blocking(&mut self, args: u64, size: usize) -> SyscallDisposition {
        let header = self
            .process
            .task
            .address_space
            .read_user_exact(args, LinuxCloneArgs::SIZE)
            .map_err(|_| SysErr::Fault);
        let Ok(header) = header else {
            return SyscallDisposition::err(SysErr::Fault);
        };
        let Some(clone_args) = LinuxCloneArgs::from_bytes(&header) else {
            return SyscallDisposition::err(SysErr::Fault);
        };
        if let Err(error) = clone_args.validate(size) {
            return SyscallDisposition::err(error);
        }

        if size > LinuxCloneArgs::SIZE {
            let trailing = self
                .process
                .task
                .address_space
                .read_user_exact(
                    args + LinuxCloneArgs::SIZE as u64,
                    size - LinuxCloneArgs::SIZE,
                )
                .map_err(|_| SysErr::Fault);
            let Ok(trailing) = trailing else {
                return SyscallDisposition::err(SysErr::Fault);
            };
            if trailing.iter().any(|byte| *byte != 0) {
                return SyscallDisposition::err(SysErr::Inval);
            }
        }

        self.syscall_clone_process_blocking(CloneParams::from_clone3(clone_args))
    }
}
