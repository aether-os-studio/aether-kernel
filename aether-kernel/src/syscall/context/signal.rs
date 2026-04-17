use crate::errno::SysResult;
use crate::syscall::SyscallDisposition;

pub trait SignalSyscallContext {
    fn rt_sigaction(
        &mut self,
        signal: u64,
        act: u64,
        oldact: u64,
        sigsetsize: u64,
    ) -> SysResult<u64>;
    fn futex(
        &mut self,
        uaddr: u64,
        operation: u64,
        val: u64,
        timeout: u64,
        uaddr2: u64,
        val3: u64,
    ) -> SysResult<u64>;
    fn futex_blocking(
        &mut self,
        uaddr: u64,
        operation: u64,
        val: u64,
        timeout: u64,
        uaddr2: u64,
        val3: u64,
    ) -> SyscallDisposition;
    fn rt_sigprocmask(
        &mut self,
        how: u64,
        set: u64,
        oldset: u64,
        sigsetsize: u64,
    ) -> SysResult<u64>;
    fn rt_sigsuspend(&mut self, mask: u64, sigsetsize: u64) -> SysResult<u64>;
    fn rt_sigsuspend_blocking(&mut self, mask: u64, sigsetsize: u64) -> SyscallDisposition;
    fn rt_sigreturn(&mut self) -> SysResult<u64>;
}
