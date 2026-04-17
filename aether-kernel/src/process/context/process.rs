use super::*;
use crate::arch::ArchContext;

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(super) fn syscall_log_unimplemented(
        &mut self,
        number: u64,
        name: &str,
        args: crate::syscall::SyscallArgs,
    ) {
        self.services
            .log_unimplemented(number, name, self.process.identity.pid, args);
        self.process
            .task
            .process
            .context_mut()
            .set_return_value(SysErr::NoSys.errno() as u64);
    }

    pub(super) fn syscall_log_unimplemented_command(
        &mut self,
        name: &str,
        command_name: &str,
        command: u64,
        args: crate::syscall::SyscallArgs,
    ) {
        self.services.log_unimplemented_command(
            name,
            command_name,
            command,
            self.process.identity.pid,
            args,
        );
        self.process
            .task
            .process
            .context_mut()
            .set_return_value(SysErr::NoSys.errno() as u64);
    }
}
