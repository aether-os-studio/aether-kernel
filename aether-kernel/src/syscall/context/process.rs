use alloc::string::String;
use alloc::vec::Vec;

use crate::errno::SysResult;
use crate::process::CloneParams;
use crate::syscall::SyscallDisposition;

pub trait ProcessSyscallOps {
    fn geteuid(&self) -> SysResult<u64>;
    fn getegid(&self) -> SysResult<u64>;
    fn getuid(&self) -> SysResult<u64>;
    fn getgid(&self) -> SysResult<u64>;
    fn getresuid(&mut self, ruid: u64, euid: u64, suid: u64) -> SysResult<u64>;
    fn getresgid(&mut self, rgid: u64, egid: u64, sgid: u64) -> SysResult<u64>;
    fn getppid(&self) -> SysResult<u64>;
    fn gettid(&self) -> SysResult<u64>;
    fn setuid(&mut self, uid: u64) -> SysResult<u64>;
    fn setgid(&mut self, gid: u64) -> SysResult<u64>;
    fn setresuid(&mut self, ruid: u64, euid: u64, suid: u64) -> SysResult<u64>;
    fn setresgid(&mut self, rgid: u64, egid: u64, sgid: u64) -> SysResult<u64>;
    fn setgroups(&mut self, size: usize, list: u64) -> SysResult<u64>;
    fn fork(&mut self, flags: u64) -> SysResult<u64>;
    fn vfork_blocking(&mut self) -> SyscallDisposition;
    fn clone_process(&mut self, params: CloneParams) -> SysResult<u64>;
    fn clone_process_blocking(&mut self, params: CloneParams) -> SyscallDisposition;
    fn clone3(&mut self, args: u64, size: usize) -> SysResult<u64>;
    fn clone3_blocking(&mut self, args: u64, size: usize) -> SyscallDisposition;
    fn wait4(&mut self, pid: i32, status: u64, options: u64, rusage: u64) -> SysResult<u64>;
    fn wait4_blocking(
        &mut self,
        pid: i32,
        status: u64,
        options: u64,
        rusage: u64,
    ) -> SyscallDisposition;
    fn send_signal(&mut self, pid: i32, signal: u64) -> SysResult<u64>;
    fn tkill(&mut self, pid: i32, signal: u64) -> SysResult<u64>;
    fn tgkill(&mut self, tgid: i32, pid: i32, signal: u64) -> SysResult<u64>;
    fn set_tid_address(&mut self, address: u64) -> SysResult<u64>;
    fn execve(&mut self, path: &str, argv: Vec<String>, envp: Vec<String>) -> SysResult<u64>;
    fn prctl(&mut self, option: u64, arg2: u64, arg3: u64, arg4: u64, arg5: u64) -> SysResult<u64>;
    fn prlimit64(
        &mut self,
        pid: i32,
        resource: u64,
        new_limit: u64,
        old_limit: u64,
    ) -> SysResult<u64>;
    fn uname(&mut self, address: u64) -> SysResult<u64>;
    fn set_robust_list(&mut self, head: u64, len: u64) -> SysResult<u64>;
    fn rseq(&mut self, area: u64, len: u64, flags: u64, signature: u64) -> SysResult<u64>;
    fn getrandom(&mut self, address: u64, len: usize, flags: u64) -> SysResult<u64>;
}
