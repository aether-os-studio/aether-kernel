use crate::arch::syscall::nr;
use crate::errno::SysResult;
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(pub struct BindSyscall => nr::BIND, "bind", |ctx, args| {
    SyscallDisposition::Return(ctx.bind(args.get(0), args.get(1), args.get(2) as usize))
});

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_bind(
        &mut self,
        fd: u64,
        address: u64,
        address_len: usize,
    ) -> SysResult<u64> {
        let (_file_ref, socket) = self.socket_from_fd(fd)?;
        let address = self.read_socket_address(address, address_len)?;
        if let Ok(Some(path)) = crate::net::unix_pathname_from_raw(address.as_slice()) {
            self.services.bind_socket(
                &self.process.fs,
                &path,
                self.masked_mode(0o777, 0o140000),
            )?;
            if let Err(error) = socket.bind(address.as_slice()) {
                let _ = self.services.unlink(&self.process.fs, &path, 0);
                return Err(error);
            }
        } else {
            socket.bind(address.as_slice())?;
        }
        Ok(0)
    }
}
