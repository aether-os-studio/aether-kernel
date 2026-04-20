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
    fn parse_clone3_set_tid(&self, clone_args: LinuxCloneArgs) -> SysResult<Option<u32>> {
        if clone_args.set_tid_size == 0 {
            return Ok(None);
        }
        if clone_args.set_tid_size > 1 {
            // TODO: extend this once nested PID namespaces exist.
            return Err(SysErr::Inval);
        }
        if !self.process.credentials.is_superuser() {
            // TODO: replace this with capability checks once user namespaces exist.
            return Err(SysErr::Perm);
        }

        let bytes = self.syscall_read_user_exact_buffer(clone_args.set_tid, 4)?;
        let requested = i32::from_ne_bytes(bytes.as_slice().try_into().map_err(|_| SysErr::Fault)?);
        if requested <= 0 {
            return Err(SysErr::Inval);
        }
        Ok(Some(requested as u32))
    }

    pub(crate) fn syscall_clone3(&mut self, args: u64, size: usize) -> SysResult<u64> {
        let disposition = self.syscall_clone3_blocking(args, size);
        match disposition {
            SyscallDisposition::Return(result) => result,
            _ => Err(SysErr::Again),
        }
    }

    pub(crate) fn syscall_clone3_blocking(&mut self, args: u64, size: usize) -> SyscallDisposition {
        if size < LinuxCloneArgs::SIZE_VER0 {
            return SyscallDisposition::err(SysErr::Inval);
        }

        let header = self.syscall_read_user_exact_buffer(args, size.min(LinuxCloneArgs::SIZE));
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

        let requested_pid = match self.parse_clone3_set_tid(clone_args) {
            Ok(requested_pid) => requested_pid,
            Err(error) => return SyscallDisposition::err(error),
        };

        self.syscall_clone_process_blocking(CloneParams::from_clone3(clone_args, requested_pid))
    }
}
