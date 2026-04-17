use crate::arch::syscall::nr;
use crate::errno::SysResult;
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(pub struct SocketSyscall => nr::SOCKET, "socket", |ctx, args| {
    SyscallDisposition::Return(ctx.socket(args.get(0) as i32, args.get(1), args.get(2) as i32))
});

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_socket(
        &mut self,
        domain: i32,
        socket_type: u64,
        protocol: i32,
    ) -> SysResult<u64> {
        let owner = crate::net::SocketCredentials::new(
            self.process.identity.pid,
            self.process.credentials.uid,
            self.process.credentials.gid,
        );
        let created = crate::net::create_socket(domain, socket_type, protocol, owner)?;
        let filesystem = crate::process::anonymous_filesystem_identity();
        Ok(self.process.files.insert_node(
            created.node,
            created.flags,
            filesystem,
            None,
            created.cloexec,
        ) as u64)
    }
}
