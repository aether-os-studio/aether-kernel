use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct MunmapSyscall => nr::MUNMAP, "munmap", |ctx, args| {
        SyscallDisposition::Return(ctx.munmap(args.get(0), args.get(1)))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_munmap(&mut self, address: u64, len: u64) -> SysResult<u64> {
        self.process
            .task
            .address_space
            .munmap(address, len)
            .map_err(SysErr::from)?;
        if len != 0 {
            let end = address.saturating_add(
                len.div_ceil(aether_frame::mm::PAGE_SIZE) * aether_frame::mm::PAGE_SIZE,
            );
            self.process.remove_mmap_region_range(address, end);
        }
        Ok(0)
    }
}
