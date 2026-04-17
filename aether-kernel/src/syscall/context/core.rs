use crate::syscall::{BlockResult, SyscallArgs};

pub trait CoreSyscallContext {
    fn pid(&self) -> u32;
    fn take_wake_result(&mut self) -> Option<BlockResult>;
    fn has_wake_result(&self) -> bool;
}

pub trait LogSyscallContext {
    fn log_unimplemented(&mut self, number: u64, name: &str, args: SyscallArgs);
    fn log_unimplemented_command(
        &mut self,
        name: &str,
        command_name: &str,
        command: u64,
        args: SyscallArgs,
    );
}
