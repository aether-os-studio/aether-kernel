use crate::errno::SysErr;
use crate::syscall::KernelSyscallContext;

const SOCK_NONBLOCK: u64 = 0o0004000;
const SOCK_CLOEXEC: u64 = 0o2000000;

pub(super) fn validate_accept4_flags(flags: u64) -> Result<(), SysErr> {
    if (flags & !(SOCK_NONBLOCK | SOCK_CLOEXEC)) != 0 {
        return Err(SysErr::Inval);
    }
    Ok(())
}

pub(super) fn read_socklen(
    ctx: &dyn KernelSyscallContext,
    optlen_ptr: u64,
) -> Result<usize, SysErr> {
    if optlen_ptr == 0 {
        return Err(SysErr::Fault);
    }
    let bytes = ctx.read_user_buffer(optlen_ptr, 4)?;
    Ok(u32::from_ne_bytes(bytes.try_into().map_err(|_| SysErr::Fault)?) as usize)
}

pub(super) fn write_length_result(
    ctx: &mut dyn KernelSyscallContext,
    data_ptr: u64,
    len_ptr: u64,
    value: &[u8],
) -> Result<u64, SysErr> {
    let requested = read_socklen(ctx, len_ptr)?;
    if requested != 0 {
        if data_ptr == 0 {
            return Err(SysErr::Fault);
        }
        let count = core::cmp::min(requested, value.len());
        if count != 0 {
            ctx.write_user_buffer(data_ptr, &value[..count])?;
        }
    }
    ctx.write_user_buffer(len_ptr, &(value.len() as u32).to_ne_bytes())?;
    Ok(0)
}
