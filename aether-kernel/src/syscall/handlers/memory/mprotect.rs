use aether_frame::mm::PAGE_SIZE;

use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct MprotectSyscall => nr::MPROTECT, "mprotect", |ctx, args| {
        SyscallDisposition::Return(ctx.mprotect(args.get(0), args.get(1), args.get(2)))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_mprotect(&mut self, address: u64, len: u64, prot: u64) -> SysResult<u64> {
        let page_flags = crate::process::ProcessSyscallContext::<S>::mmap_page_flags(prot);

        self.process
            .task
            .address_space
            .mprotect(address, len, page_flags)
            .map_err(SysErr::from)?;
        if len != 0 {
            let end = address.saturating_add(len.div_ceil(PAGE_SIZE) * PAGE_SIZE);
            self.process
                .update_mmap_region_flags(address, end, page_flags);
        }
        Ok(0)
    }
}
