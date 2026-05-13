use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::ProcessSyscallContext;
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(
    pub struct GetcwdSyscall => nr::GETCWD, "getcwd", |ctx, args| {
        SyscallDisposition::Return(ctx.getcwd(args.get(0), args.get(1) as usize))
    }
);

impl ProcessSyscallContext<'_> {
    pub(crate) fn getcwd(&mut self, address: u64, len: usize) -> SysResult<u64> {
        let mut cwd = self.services.getcwd(&self.process.fs).into_bytes();
        cwd.push(0);
        if len < cwd.len() {
            return Err(SysErr::Inval);
        }
        self.write_user_buffer(address, &cwd)?;
        Ok(address)
    }
}
