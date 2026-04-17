use aether_frame::libs::spin::SpinLock;

use super::{KernelSyscallContext, SyscallArgs, SyscallDisposition};

const MAX_SYSCALLS: usize = 512;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SyscallEntry {
    pub number: u64,
    pub name: &'static str,
    pub handle: fn(&mut dyn KernelSyscallContext, SyscallArgs) -> SyscallDisposition,
}

type SyscallHandler = fn(&mut dyn KernelSyscallContext, SyscallArgs) -> SyscallDisposition;

pub struct SyscallDispatch {
    pub disposition: SyscallDisposition,
    pub name: &'static str,
}

struct SyscallRegistryState {
    handlers: [Option<SyscallHandler>; MAX_SYSCALLS],
    names: [Option<&'static str>; MAX_SYSCALLS],
}

pub struct SyscallRegistry {
    state: SpinLock<SyscallRegistryState>,
}

impl SyscallRegistry {
    pub const fn new() -> Self {
        Self {
            state: SpinLock::new(SyscallRegistryState {
                handlers: [None; MAX_SYSCALLS],
                names: [None; MAX_SYSCALLS],
            }),
        }
    }

    pub fn register(&self, syscall: SyscallEntry) {
        let number = syscall.number as usize;
        if number >= MAX_SYSCALLS {
            log::warn!("syscall: dropping out-of-range handler {}", syscall.name);
            return;
        }

        let mut state = self.state.lock_irqsave();
        state.handlers[number] = Some(syscall.handle);
        state.names[number] = Some(syscall.name);
    }

    #[inline(never)]
    pub fn dispatch(
        &self,
        number: u64,
        context: &mut dyn KernelSyscallContext,
        args: SyscallArgs,
    ) -> Option<SyscallDispatch> {
        let guard = self.state.lock_irqsave();
        let handle = guard.handlers.get(number as usize).copied().flatten()?;
        let name = guard.names.get(number as usize).copied().flatten()?;
        drop(guard);
        Some(SyscallDispatch {
            disposition: handle(context, args),
            name,
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
