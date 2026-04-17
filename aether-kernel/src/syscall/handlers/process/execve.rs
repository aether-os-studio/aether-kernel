use alloc::vec::Vec;

use crate::arch::syscall::nr;
use crate::errno::SysErr;
use crate::errno::SysResult;
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;
use crate::syscall::abi::read_path;

crate::declare_syscall!(
    pub struct ExecveSyscall => nr::EXECVE, "execve", |ctx, args| {
        let Ok(path) = read_path(ctx, args.get(0), 512) else {
            return SyscallDisposition::Return(Err(SysErr::Fault));
        };
        let Ok(argv_ptrs) = ctx.read_user_pointer_array(args.get(1), 256) else {
            return SyscallDisposition::Return(Err(SysErr::Fault));
        };
        let Ok(envp_ptrs) = ctx.read_user_pointer_array(args.get(2), 256) else {
            return SyscallDisposition::Return(Err(SysErr::Fault));
        };
        let Ok(argv) = super::read_string_vector(ctx, &argv_ptrs) else {
            return SyscallDisposition::Return(Err(SysErr::Fault));
        };
        let Ok(envp) = super::read_string_vector(ctx, &envp_ptrs) else {
            return SyscallDisposition::Return(Err(SysErr::Fault));
        };

        SyscallDisposition::Return(ctx.execve(&path, argv, envp))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_execve(
        &mut self,
        path: &str,
        argv: Vec<alloc::string::String>,
        envp: Vec<alloc::string::String>,
    ) -> SysResult<u64> {
        let result = self.services.execve(self.process, path, argv, envp);
        if result.is_ok() {
            self.process.files.close_cloexec();
            if let Some(parent) = self.process.vfork_parent.take() {
                self.services
                    .wake_vfork_parent(parent, self.process.identity.pid);
            }
        }
        result
    }
}
