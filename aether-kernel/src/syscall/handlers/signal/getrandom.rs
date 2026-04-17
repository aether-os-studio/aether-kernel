use alloc::vec;

use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::{KernelSyscallContext, SyscallDisposition};

crate::declare_syscall!(
    pub struct GetrandomSyscall => nr::GETRANDOM, "getrandom", |ctx, args| {
        SyscallDisposition::Return(ctx.getrandom(args.get(0), args.get(1) as usize, args.get(2)))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_getrandom(
        &mut self,
        address: u64,
        len: usize,
        flags: u64,
    ) -> SysResult<u64> {
        const GRND_NONBLOCK: u64 = 0x1;
        const GRND_RANDOM: u64 = 0x2;
        const GRND_INSECURE: u64 = 0x4;

        if (flags & !(GRND_NONBLOCK | GRND_RANDOM | GRND_INSECURE)) != 0 {
            return Err(SysErr::Inval);
        }
        let mut bytes = vec![0u8; len];
        let mut seed = aether_frame::interrupt::timer::ticks();
        for byte in &mut bytes {
            seed ^= seed.rotate_left(13).wrapping_add(0x9e37_79b9_7f4a_7c15);
            *byte = seed as u8;
        }
        self.write_user_buffer(address, &bytes)?;
        Ok(len as u64)
    }
}
