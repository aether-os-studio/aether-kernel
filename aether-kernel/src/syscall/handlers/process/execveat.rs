use crate::arch::syscall::nr;
use crate::errno::SysErr;
use crate::syscall::SyscallDisposition;
use crate::syscall::abi::arg_i64_from_i32;

const AT_FDCWD: i64 = -100;
const AT_EMPTY_PATH: u64 = 0x1000;

crate::declare_syscall!(
    pub struct ExecveAtSyscall => nr::EXECVEAT, "execveat", |ctx, args| {
        let dirfd = arg_i64_from_i32(args.get(0));
        let flags = args.get(4);
        if dirfd != AT_FDCWD || (flags & !AT_EMPTY_PATH) != 0 {
            ctx.log_unimplemented_command("execveat", "flags", flags, args);
            return SyscallDisposition::Return(Err(SysErr::NoSys));
        }

        let Ok(path) = ctx.read_user_c_string(args.get(1), 512) else {
            return SyscallDisposition::Return(Err(SysErr::Fault));
        };
        let Ok(argv_ptrs) = ctx.read_user_pointer_array(args.get(2), 256) else {
            return SyscallDisposition::Return(Err(SysErr::Fault));
        };
        let Ok(envp_ptrs) = ctx.read_user_pointer_array(args.get(3), 256) else {
            return SyscallDisposition::Return(Err(SysErr::Fault));
        };
        let Ok(argv) = super::read_string_vector(ctx, &argv_ptrs) else {
            return SyscallDisposition::Return(Err(SysErr::Fault));
        };
        let Ok(envp) = super::read_string_vector(ctx, &envp_ptrs) else {
            return SyscallDisposition::Return(Err(SysErr::Fault));
        };

        SyscallDisposition::Return(ctx.execve(&path, argv, envp))
    }
);
