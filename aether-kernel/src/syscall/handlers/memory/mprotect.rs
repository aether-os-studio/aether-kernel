use aether_frame::mm::MapFlags;

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
        const PROT_WRITE: u64 = 0x2;
        const PROT_EXEC: u64 = 0x4;

        let mut page_flags = MapFlags::READ | MapFlags::USER;
        if (prot & PROT_WRITE) != 0 {
            page_flags = page_flags | MapFlags::WRITE;
        }
        if (prot & PROT_EXEC) != 0 {
            page_flags = page_flags | MapFlags::EXECUTE;
        }

        self.process
            .task
            .address_space
            .mprotect(address, len, page_flags)
            .map(|_| 0)
            .map_err(SysErr::from)
    }
}
