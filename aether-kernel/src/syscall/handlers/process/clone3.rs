use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{CloneParams, LinuxCloneArgs, ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct Clone3Syscall => nr::CLONE3, "clone3", |ctx, args| {
        SyscallDisposition::Return(ctx.clone3(args.get(0), args.get(1) as usize))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_clone3(&mut self, args: u64, size: usize) -> SysResult<u64> {
        let header = self
            .process
            .task
            .address_space
            .read_user_exact(args, LinuxCloneArgs::SIZE)
            .map_err(|_| SysErr::Fault)?;
        let clone_args = LinuxCloneArgs::from_bytes(&header).ok_or(SysErr::Fault)?;
        clone_args.validate(size)?;

        if size > LinuxCloneArgs::SIZE {
            let trailing = self
                .process
                .task
                .address_space
                .read_user_exact(
                    args + LinuxCloneArgs::SIZE as u64,
                    size - LinuxCloneArgs::SIZE,
                )
                .map_err(|_| SysErr::Fault)?;
            if trailing.iter().any(|byte| *byte != 0) {
                return Err(SysErr::Inval);
            }
        }

        self.syscall_clone_process(CloneParams::from_clone3(clone_args))
    }
}
