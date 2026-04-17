use crate::arch::syscall::nr;
use crate::errno::SysErr;
use crate::errno::SysResult;
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct MountSyscall => nr::MOUNT, "mount", |ctx, args| {
        let source_ptr = args.get(0);
        let target_ptr = args.get(1);
        let fstype_ptr = args.get(2);
        let flags = args.get(3);

        let source = if source_ptr == 0 {
            None
        } else {
            match ctx.read_user_c_string(source_ptr, 256) {
                Ok(value) => Some(value),
                Err(error) => return SyscallDisposition::Return(Err(error)),
            }
        };
        let Ok(target) = ctx.read_user_c_string(target_ptr, 256) else {
            return SyscallDisposition::Return(Err(SysErr::Fault));
        };
        let fstype = if fstype_ptr == 0 {
            None
        } else {
            match ctx.read_user_c_string(fstype_ptr, 64) {
                Ok(value) => Some(value),
                Err(error) => return SyscallDisposition::Return(Err(error)),
            }
        };

        SyscallDisposition::Return(ctx.mount(source.as_deref(), &target, fstype.as_deref(), flags))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_mount(
        &mut self,
        source: Option<&str>,
        target: &str,
        fstype: Option<&str>,
        flags: u64,
    ) -> SysResult<u64> {
        self.services
            .mount(&mut self.process.fs, source, target, fstype, flags)
    }

    pub(crate) fn syscall_umount(&mut self, target: &str, flags: u64) -> SysResult<u64> {
        self.services.umount(&self.process.fs, target, flags)
    }
}
