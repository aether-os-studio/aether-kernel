use spin::Once;

use super::{KernelSyscallContext, SyscallArgs, SyscallDisposition};

const MAX_SYSCALLS: usize = 512;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SyscallEntry {
    pub number: u64,
    pub name: &'static str,
    pub handle: fn(&mut dyn KernelSyscallContext, SyscallArgs) -> SyscallDisposition,
}

pub struct SyscallDispatch {
    pub disposition: SyscallDisposition,
    pub name: &'static str,
}

pub struct SyscallRegistry {
    entries: [Once<SyscallEntry>; MAX_SYSCALLS],
}

impl SyscallRegistry {
    pub const fn new() -> Self {
        Self {
            entries: [const { Once::new() }; MAX_SYSCALLS],
        }
    }

    pub fn register(&self, syscall: SyscallEntry) {
        let number = syscall.number as usize;
        if number >= MAX_SYSCALLS {
            log::warn!("syscall: dropping out-of-range handler {}", syscall.name);
            return;
        }

        self.entries[number].call_once(|| syscall);
    }

    #[inline(never)]
    pub fn dispatch(
        &self,
        number: u64,
        context: &mut dyn KernelSyscallContext,
        args: SyscallArgs,
    ) -> Option<SyscallDispatch> {
        let entry = self.entries.get(number as usize)?.get()?;
        Some(SyscallDispatch {
            disposition: (entry.handle)(context, args),
            name: entry.name,
        })
    }
}

static REGISTRY: SyscallRegistry = SyscallRegistry::new();

pub fn registry() -> &'static SyscallRegistry {
    &REGISTRY
}

#[inline(never)]
pub fn dispatch(
    number: u64,
    context: &mut dyn KernelSyscallContext,
    args: SyscallArgs,
) -> Option<SyscallDispatch> {
    REGISTRY.dispatch(number, context, args)
}
