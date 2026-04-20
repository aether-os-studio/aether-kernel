use crate::errno::SysResult;

pub trait MemorySyscallContext {
    fn brk(&mut self, address: u64) -> SysResult<u64>;
    fn mmap(
        &mut self,
        address: u64,
        len: u64,
        prot: u64,
        flags: u64,
        fd: u64,
        offset: u64,
    ) -> SysResult<u64>;
    fn munmap(&mut self, address: u64, len: u64) -> SysResult<u64>;
    fn mprotect(&mut self, address: u64, len: u64, prot: u64) -> SysResult<u64>;
    fn mremap(
        &mut self,
        old_address: u64,
        old_size: u64,
        new_size: u64,
        flags: u64,
        new_address: u64,
    ) -> SysResult<u64>;
    fn mincore(&mut self, address: u64, len: u64, vec: u64) -> SysResult<u64>;
}
