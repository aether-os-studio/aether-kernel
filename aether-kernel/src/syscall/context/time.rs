use crate::errno::SysResult;
use crate::syscall::SyscallDisposition;

pub trait TimeSyscallContext {
    fn gettimeofday(&mut self, tv: u64, tz: u64) -> SysResult<u64>;
    fn clock_gettime(&mut self, clock_id: u64, tp: u64) -> SysResult<u64>;
    fn clock_getres(&mut self, clock_id: u64, tp: u64) -> SysResult<u64>;
    fn clock_nanosleep(
        &mut self,
        clock_id: u64,
        flags: u64,
        rqtp: u64,
        rmtp: u64,
    ) -> SysResult<u64>;
    fn clock_nanosleep_blocking(
        &mut self,
        clock_id: u64,
        flags: u64,
        rqtp: u64,
        rmtp: u64,
    ) -> SyscallDisposition;
}
