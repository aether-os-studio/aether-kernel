use crate::arch::syscall::nr;
use crate::errno::SysResult;
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::KernelSyscallContext;
use crate::syscall::SyscallDisposition;

crate::declare_syscall!(pub struct SocketpairSyscall => nr::SOCKETPAIR, "socketpair", |ctx, args| {
    SyscallDisposition::Return(ctx.socketpair(args.get(0) as i32, args.get(1), args.get(2) as i32, args.get(3)))
});

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_socketpair(
        &mut self,
        domain: i32,
        socket_type: u64,
        protocol: i32,
        sv: u64,
    ) -> SysResult<u64> {
        let owner = crate::net::SocketCredentials::new(
            self.process.identity.pid,
            self.process.credentials.uid,
            self.process.credentials.gid,
        );
        let created = crate::net::create_socket_pair(domain, socket_type, protocol, owner)?;
        let filesystem = crate::process::anonymous_filesystem_identity();
        let first = self.process.files.insert_node(
            created.first,
            created.flags,
            filesystem,
            None,
            created.cloexec,
        );
        let second = self.process.files.insert_node(
            created.second,
            created.flags,
            filesystem,
            None,
            created.cloexec,
        );

        let mut bytes = [0u8; 8];
        bytes[..4].copy_from_slice(&(first as i32).to_ne_bytes());
        bytes[4..].copy_from_slice(&(second as i32).to_ne_bytes());
        self.write_user_buffer(sv, &bytes)?;
        Ok(0)
    }
}
