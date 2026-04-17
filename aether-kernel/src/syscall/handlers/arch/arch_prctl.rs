use crate::arch::{ArchContext, syscall::nr};
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::{KernelSyscallContext, SyscallDisposition};

crate::declare_syscall!(
    pub struct ArchPrctlSyscall => nr::ARCH_PRCTL, "arch_prctl", |ctx, args| {
        SyscallDisposition::Return(ctx.arch_prctl(args.get(0), args.get(1)))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_arch_prctl(&mut self, code: u64, address: u64) -> SysResult<u64> {
        const ARCH_SET_GS: u64 = 0x1001;
        const ARCH_SET_FS: u64 = 0x1002;
        const ARCH_GET_FS: u64 = 0x1003;
        const ARCH_GET_GS: u64 = 0x1004;

        let context = self.process.task.process.context_mut();
        match code {
            ARCH_SET_FS => {
                context.set_thread_pointer(address);
                Ok(0)
            }
            ARCH_SET_GS => {
                context.set_secondary_thread_pointer(address);
                Ok(0)
            }
            ARCH_GET_FS => {
                let value = context.thread_pointer().to_ne_bytes();
                let _ = context;
                self.write_user_buffer(address, &value)?;
                Ok(0)
            }
            ARCH_GET_GS => {
                let value = context.secondary_thread_pointer().to_ne_bytes();
                let _ = context;
                self.write_user_buffer(address, &value)?;
                Ok(0)
            }
            _ => Err(SysErr::Inval),
        }
    }
}
