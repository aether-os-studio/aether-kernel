use crate::arch::syscall::nr;
use crate::errno::SysErr;
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(pub struct FsopenSyscall => nr::FSOPEN, "fsopen", |_ctx, _args| {
    // TODO: Linux fsopen(2) needs the full new mount API object model
    // (fsopen/fsconfig/fsmount/move_mount/open_tree). Until those kernel
    // subsystems exist, return ENOSYS explicitly instead of falling through
    // the generic unknown-syscall path.
    SyscallDisposition::err(SysErr::NoSys)
});
