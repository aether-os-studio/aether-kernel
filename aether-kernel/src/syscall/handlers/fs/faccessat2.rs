use crate::arch::syscall::nr;
use crate::syscall::SyscallDisposition;
use crate::syscall::abi::arg_i64_from_i32;

crate::declare_syscall!(
    pub struct FaccessAt2Syscall => nr::FACCESSAT2, "faccessat2", |ctx, args| {
        const AT_EMPTY_PATH: u64 = 0x1000;

        let dirfd = arg_i64_from_i32(args.get(0));
        let pathname = args.get(1);
        let path = if pathname == 0 {
            if (args.get(3) & AT_EMPTY_PATH) == 0 {
                return SyscallDisposition::Return(Err(crate::errno::SysErr::Fault));
            }
            alloc::string::String::new()
        } else {
            let Ok(path) = ctx.read_user_c_string(pathname, 512) else {
                return SyscallDisposition::Return(Err(crate::errno::SysErr::Fault));
            };
            path
        };

        SyscallDisposition::Return(ctx.faccessat(dirfd, &path, args.get(2), args.get(3)))
    }
);
