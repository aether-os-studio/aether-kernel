use crate::errno::SysResult;

pub trait ArchSyscallContext {
    fn iopl(&mut self, level: u64) -> SysResult<u64>;
    fn arch_prctl(&mut self, code: u64, address: u64) -> SysResult<u64>;
}
