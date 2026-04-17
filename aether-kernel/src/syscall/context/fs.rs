use crate::errno::SysResult;

pub trait FsSyscallContext {
    fn access(&mut self, path: &str, mode: u64) -> SysResult<u64>;
    fn faccessat(&mut self, dirfd: i64, path: &str, mode: u64, flags: u64) -> SysResult<u64>;
    fn openat(&mut self, dirfd: i64, path: &str, flags: u64, mode: u64) -> SysResult<u64>;
    fn creat(&mut self, path: &str, mode: u64) -> SysResult<u64>;
    fn symlink(&mut self, target: &str, linkpath: &str) -> SysResult<u64>;
    fn unlinkat(&mut self, dirfd: i64, path: &str, flags: u64) -> SysResult<u64>;
    fn readlinkat(&mut self, dirfd: i64, path: &str, address: u64, len: usize) -> SysResult<u64>;
    fn newfstatat(&mut self, dirfd: i64, path: &str, address: u64, flags: u64) -> SysResult<u64>;
    fn statx(
        &mut self,
        dirfd: i64,
        path: &str,
        flags: u64,
        mask: u64,
        address: u64,
    ) -> SysResult<u64>;
    fn getcwd(&mut self, address: u64, len: usize) -> SysResult<u64>;
    fn chdir(&mut self, path: &str) -> SysResult<u64>;
    fn chroot(&mut self, path: &str) -> SysResult<u64>;
    fn chown(&mut self, path: &str, owner: u64, group: u64) -> SysResult<u64>;
    fn lchown(&mut self, path: &str, owner: u64, group: u64) -> SysResult<u64>;
    fn fchown(&mut self, fd: u64, owner: u64, group: u64) -> SysResult<u64>;
    fn fchownat(
        &mut self,
        dirfd: i64,
        path: &str,
        owner: u64,
        group: u64,
        flags: u64,
    ) -> SysResult<u64>;
    fn mkdir(&mut self, path: &str, mode: u64) -> SysResult<u64>;
    fn rename(&mut self, old_path: &str, new_path: &str) -> SysResult<u64>;
    fn renameat(
        &mut self,
        olddirfd: i64,
        old_path: &str,
        newdirfd: i64,
        new_path: &str,
    ) -> SysResult<u64>;
    fn stat_path(&mut self, path: &str, address: u64) -> SysResult<u64>;
    fn lstat_path(&mut self, path: &str, address: u64) -> SysResult<u64>;
    fn statfs_path(&mut self, path: &str, address: u64) -> SysResult<u64>;
    fn umask(&mut self, mask: u64) -> SysResult<u64>;
    fn mount(
        &mut self,
        source: Option<&str>,
        target: &str,
        fstype: Option<&str>,
        flags: u64,
    ) -> SysResult<u64>;
    fn umount(&mut self, target: &str, flags: u64) -> SysResult<u64>;
    fn pivot_root(&mut self, new_root: &str, put_old: &str) -> SysResult<u64>;
}
