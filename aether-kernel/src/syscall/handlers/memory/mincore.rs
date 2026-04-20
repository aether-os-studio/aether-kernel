use aether_frame::mm::PAGE_SIZE;
use alloc::vec;

use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::{KernelSyscallContext, SyscallDisposition};

crate::declare_syscall!(
    pub struct MincoreSyscall => nr::MINCORE, "mincore", |ctx, args| {
        SyscallDisposition::Return(ctx.mincore(args.get(0), args.get(1), args.get(2)))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_mincore(&mut self, address: u64, len: u64, vec: u64) -> SysResult<u64> {
        if len == 0 {
            return Ok(0);
        }
        if !address.is_multiple_of(PAGE_SIZE) {
            return Err(SysErr::Inval);
        }
        if vec == 0 {
            return Err(SysErr::Fault);
        }

        let page_count = len.div_ceil(PAGE_SIZE) as usize;
        let end = address
            .checked_add(page_count as u64 * PAGE_SIZE)
            .ok_or(SysErr::NoMem)?;
        let mut status = vec![0u8; page_count];

        for (index, page_base) in (address..end).step_by(PAGE_SIZE as usize).enumerate() {
            if self
                .process
                .covering_mmap_region(page_base, page_base.saturating_add(1))
                .is_none()
            {
                return Err(SysErr::NoMem);
            }

            // TODO: This currently reports "mapped" as "resident". Linux mincore(2) is about
            // page-cache / page-table residency, which needs real per-page presence tracking.
            status[index] = 1;
        }

        self.write_user_buffer(vec, &status)?;
        Ok(0)
    }
}
