pub mod abi;
mod handlers;
mod registry;

use crate::arch::ArchContext;
use crate::errno::{SysErr, SysResult};
use crate::process::{FutexKey, Pid, ProcessSyscallContext, WaitChildApi, WaitChildSelector};
use aether_vfs::PollEvents;

pub use self::registry::{SyscallDispatch, SyscallEntry};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyscallArgs {
    raw: [u64; 6],
}

impl SyscallArgs {
    pub fn from_context(context: &impl ArchContext) -> Self {
        Self {
            raw: context.syscall_args(),
        }
    }

    pub fn get(self, index: usize) -> u64 {
        self.raw.get(index).copied().unwrap_or(0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockType {
    Timer {
        target_nanos: u64,
        request_nanos: u64,
        rmtp: u64,
        flags: u64,
    },
    File {
        fd: u32,
        events: PollEvents,
    },
    Poll {
        deadline_nanos: Option<u64>,
    },
    Futex {
        key: FutexKey,
        bitset: u32,
        deadline_nanos: Option<u64>,
    },
    SignalSuspend,
    Vfork {
        child: Pid,
    },
    WaitChild {
        selector: WaitChildSelector,
        api: WaitChildApi,
        status_ptr: u64,
        info_ptr: u64,
        options: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockResult {
    Timer {
        completed: bool,
        remaining_nanos: u64,
        rmtp: u64,
        is_absolute: bool,
    },
    File {
        ready: bool,
    },
    Poll {
        timed_out: bool,
    },
    Futex {
        woke: bool,
        timed_out: bool,
    },
    SignalInterrupted,
    CompletedValue {
        value: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyscallDisposition {
    Return(SysResult<u64>),
    Exit(i32),
    ExitGroup(i32),
}

impl SyscallDisposition {
    pub fn ok(value: u64) -> Self {
        Self::Return(Ok(value))
    }

    pub fn err(error: SysErr) -> Self {
        Self::Return(Err(error))
    }
}

#[macro_export]
macro_rules! declare_syscall {
    ($(#[$meta:meta])* $vis:vis struct $name:ident => $number:expr, $label:expr, |$ctx:ident, $args:ident| $body:block) => {
        $(#[$meta])*
        $vis struct $name;

        impl $name {
            fn handle(
                context: &mut $crate::process::ProcessSyscallContext<'_>,
                args: $crate::syscall::SyscallArgs,
            ) -> $crate::syscall::SyscallDisposition {
                let $ctx = context;
                let $args = args;
                $body
            }

            $vis const ENTRY: $crate::syscall::SyscallEntry = $crate::syscall::SyscallEntry {
                number: $number,
                name: $label,
                handle: Self::handle,
            };
        }
    };
}

#[macro_export]
macro_rules! register_syscalls {
    ($registry:expr, [$($handler:path),* $(,)?]) => {{
        $( $registry.register(<$handler>::ENTRY); )*
    }};
}

pub fn init() {
    handlers::init();
}

pub(crate) fn dispatch(
    number: u64,
    context: &mut ProcessSyscallContext<'_>,
    args: SyscallArgs,
) -> SyscallDispatch {
    registry::dispatch(number, context, args)
        .inspect(|dispatch| {
            if matches!(
                dispatch.disposition,
                SyscallDisposition::Return(Err(SysErr::NoSys))
            ) {
                context.log_unimplemented(number, dispatch.name, args);
            }
        })
        .unwrap_or_else(|| {
            context.log_unimplemented(number, "unknown", args);
            SyscallDispatch {
                disposition: SyscallDisposition::Return(Err(SysErr::NoSys)),
                name: "unknown",
            }
        })
}
