use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct SetGroupsSyscall => nr::SETGROUPS, "setgroups", |ctx, args| {
        SyscallDisposition::Return(ctx.setgroups(args.get(0) as usize, args.get(1)))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_setgroups(&mut self, size: usize, list: u64) -> SysResult<u64> {
        const NGROUPS_MAX: usize = 65536;

        if size > NGROUPS_MAX {
            return Err(SysErr::Inval);
        }
        if !self.process.credentials.is_superuser() {
            return Err(SysErr::Perm);
        }
        if size == 0 {
            self.process.credentials.supplementary_groups.clear();
            return Ok(0);
        }
        if list == 0 {
            return Err(SysErr::Fault);
        }

        let bytes = self.syscall_read_user_exact_buffer(
            list,
            size.checked_mul(core::mem::size_of::<u32>())
                .ok_or(SysErr::Inval)?,
        )?;
        let mut groups = alloc::vec::Vec::with_capacity(size);
        for chunk in bytes.chunks_exact(core::mem::size_of::<u32>()) {
            groups.push(u32::from_ne_bytes(
                chunk.try_into().map_err(|_| SysErr::Fault)?,
            ));
        }
        self.process.credentials.supplementary_groups = groups;
        Ok(0)
    }
}
