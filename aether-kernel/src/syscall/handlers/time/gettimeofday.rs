use aether_frame::time;

use crate::arch::syscall::nr;
use crate::errno::SysResult;
use crate::process::ProcessSyscallContext;
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct GettimeofdaySyscall => nr::GETTIMEOFDAY, "gettimeofday", |ctx, args| {
        let tv = args.get(0);
        let tz = args.get(1);
        SyscallDisposition::Return(ctx.gettimeofday(tv, tz))
    }
);

impl ProcessSyscallContext<'_> {
    pub(crate) fn gettimeofday(&mut self, tv: u64, tz: u64) -> SysResult<u64> {
        if tz != 0 {
            let tz_bytes = [0u8; 8];
            self.write_user_buffer(tz, &tz_bytes)?;
        }

        if tv != 0 {
            let (secs, nanos) = time::realtime_nanos();
            let tv_sec_bytes = secs.to_ne_bytes();
            let tv_usec_bytes = ((nanos as u64) / 1000).to_ne_bytes();
            let mut tv_bytes = [0u8; 16];
            tv_bytes[..8].copy_from_slice(&tv_sec_bytes);
            tv_bytes[8..].copy_from_slice(&tv_usec_bytes);
            self.write_user_buffer(tv, &tv_bytes)?;
        }

        Ok(0)
    }
}
