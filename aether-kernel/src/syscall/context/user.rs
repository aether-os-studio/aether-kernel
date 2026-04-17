use alloc::string::String;
use alloc::vec::Vec;

use crate::errno::SysResult;

pub trait UserMemorySyscallContext {
    fn read_user_c_string(&self, address: u64, limit: usize) -> SysResult<String>;
    fn read_user_buffer(&self, address: u64, len: usize) -> SysResult<Vec<u8>>;
    fn read_user_pointer_array(&self, address: u64, limit: usize) -> SysResult<Vec<u64>>;
    fn write_user_buffer(&mut self, address: u64, bytes: &[u8]) -> SysResult<()>;
    fn write_user_timespec(&mut self, address: u64, secs: i64, nanos: i64) -> SysResult<()>;
}
