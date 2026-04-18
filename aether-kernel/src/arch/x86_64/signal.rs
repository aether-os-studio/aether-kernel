extern crate alloc;

use alloc::vec::Vec;

use aether_frame::process::UserContext;

use crate::errno::{SysErr, SysResult};
use crate::process::KernelProcess;
use crate::signal::{SignalAction, SignalInfo};

const SA_NODEFER: u64 = 0x4000_0000;
const SA_RESTORER: u64 = 0x0400_0000;
const X86_64_RED_ZONE_SIZE: u64 = 128;
const X86_64_SIGNAL_FRAME_ALIGN: u64 = 16;
const TRAMPOLINE_BYTES: [u8; 9] = [0xb8, 15, 0, 0, 0, 0x0f, 0x05, 0x0f, 0x0b];
const CONTEXT_WORDS: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalFrameError {
    InvalidFrame,
    UserMemory,
}

pub fn supports_user_handlers() -> bool {
    true
}

pub fn deliver_signal_to_user(
    process: &mut KernelProcess,
    signal: SignalInfo,
    action: SignalAction,
) -> Result<(), SignalFrameError> {
    if action.handler == 0 || action.handler == 1 {
        return Err(SignalFrameError::InvalidFrame);
    }

    let saved = *process.task.process.context();
    let saved_mask = process.signals.blocked();

    let metadata = SignalFrameLayout::new(saved.general.rsp, action)?;
    let siginfo_bytes = serialize_siginfo(signal);
    let ucontext_bytes = serialize_ucontext(saved, saved_mask);

    write_user(
        &mut process.task.address_space,
        metadata.frame_base,
        &metadata.return_address,
    )?;
    write_user(
        &mut process.task.address_space,
        metadata.context_addr,
        &serialize_context(saved),
    )?;
    write_user(
        &mut process.task.address_space,
        metadata.sigmask_addr,
        &saved_mask.to_ne_bytes(),
    )?;
    write_user(
        &mut process.task.address_space,
        metadata.siginfo_addr,
        &siginfo_bytes,
    )?;
    write_user(
        &mut process.task.address_space,
        metadata.ucontext_addr,
        &ucontext_bytes,
    )?;
    if metadata.uses_stack_trampoline {
        write_user(
            &mut process.task.address_space,
            metadata.trampoline_addr,
            &TRAMPOLINE_BYTES,
        )?;
    }

    let context = process.task.process.context_mut();
    context.general.rip = action.handler;
    context.general.rsp = metadata.frame_base;
    context.general.rdi = signal.signal as u64;
    context.general.rsi = metadata.siginfo_addr;
    context.general.rdx = metadata.ucontext_addr;

    let mut next_mask = saved_mask | action.mask;
    if (action.flags & SA_NODEFER) == 0 {
        next_mask |= crate::signal::sigbit(signal.signal);
    }
    process.signals.set_blocked_mask(next_mask);
    Ok(())
}

pub fn restore_signal_from_user(process: &mut KernelProcess) -> SysResult<u64> {
    let rsp = process.task.process.context().general.rsp;
    let frame_base = rsp.checked_sub(8).ok_or(SysErr::Fault)?;
    let layout = SignalFrameLayout::from_frame_base(frame_base);

    let context = decode_context(
        &process
            .task
            .address_space
            .read_user_exact(layout.context_addr, CONTEXT_WORDS * 8)
            .map_err(|_| SysErr::Fault)?,
    )
    .ok_or(SysErr::Fault)?;
    let mask_bytes = process
        .task
        .address_space
        .read_user_exact(layout.sigmask_addr, 8)
        .map_err(|_| SysErr::Fault)?;
    let mut raw = [0u8; 8];
    raw.copy_from_slice(&mask_bytes);
    process.signals.restore_mask(u64::from_ne_bytes(raw));
    *process.task.process.context_mut() = context;
    Ok(context.general.rax)
}

struct SignalFrameLayout {
    frame_base: u64,
    context_addr: u64,
    sigmask_addr: u64,
    siginfo_addr: u64,
    ucontext_addr: u64,
    trampoline_addr: u64,
    uses_stack_trampoline: bool,
    return_address: [u8; 8],
}

impl SignalFrameLayout {
    fn new(user_rsp: u64, action: SignalAction) -> Result<Self, SignalFrameError> {
        let frame_bytes = 8
            + (CONTEXT_WORDS as u64 * 8)
            + 8
            + 16
            + ((CONTEXT_WORDS as u64 * 8) + 8)
            + TRAMPOLINE_BYTES.len() as u64;
        let stack_top = user_rsp
            .checked_sub(X86_64_RED_ZONE_SIZE)
            .ok_or(SignalFrameError::InvalidFrame)?;
        let frame_base = align_signal_stack(
            stack_top
                .checked_sub(frame_bytes)
                .ok_or(SignalFrameError::InvalidFrame)?,
        );
        let trampoline_addr = frame_base + frame_bytes - TRAMPOLINE_BYTES.len() as u64;
        let uses_stack_trampoline = (action.flags & SA_RESTORER) == 0 || action.restorer == 0;
        let return_address = if uses_stack_trampoline {
            trampoline_addr.to_ne_bytes()
        } else {
            action.restorer.to_ne_bytes()
        };
        Ok(Self {
            frame_base,
            context_addr: frame_base + 8,
            sigmask_addr: frame_base + 8 + (CONTEXT_WORDS as u64 * 8),
            siginfo_addr: frame_base + 16 + (CONTEXT_WORDS as u64 * 8),
            ucontext_addr: frame_base + 32 + (CONTEXT_WORDS as u64 * 8),
            trampoline_addr,
            uses_stack_trampoline,
            return_address,
        })
    }

    fn from_frame_base(frame_base: u64) -> Self {
        let trampoline_addr =
            frame_base + 8 + (CONTEXT_WORDS as u64 * 8) + 8 + 16 + ((CONTEXT_WORDS as u64 * 8) + 8);
        Self {
            frame_base,
            context_addr: frame_base + 8,
            sigmask_addr: frame_base + 8 + (CONTEXT_WORDS as u64 * 8),
            siginfo_addr: frame_base + 16 + (CONTEXT_WORDS as u64 * 8),
            ucontext_addr: frame_base + 32 + (CONTEXT_WORDS as u64 * 8),
            trampoline_addr,
            uses_stack_trampoline: true,
            return_address: trampoline_addr.to_ne_bytes(),
        }
    }
}

fn serialize_context(context: UserContext) -> Vec<u8> {
    let words = [
        context.general.r15,
        context.general.r14,
        context.general.r13,
        context.general.r12,
        context.general.r11,
        context.general.r10,
        context.general.r9,
        context.general.r8,
        context.general.rdi,
        context.general.rsi,
        context.general.rbp,
        context.general.rbx,
        context.general.rdx,
        context.general.rcx,
        context.general.rax,
        context.general.rip,
        context.general.rsp,
        context.general.rflags,
        context.fs_base,
        context.gs_base,
    ];

    let mut bytes = Vec::with_capacity(words.len() * 8);
    for word in words {
        bytes.extend_from_slice(&word.to_ne_bytes());
    }
    bytes
}

fn decode_context(bytes: &[u8]) -> Option<UserContext> {
    if bytes.len() != CONTEXT_WORDS * 8 {
        return None;
    }

    let mut words = [0u64; CONTEXT_WORDS];
    for (index, chunk) in bytes.chunks_exact(8).enumerate() {
        let mut raw = [0u8; 8];
        raw.copy_from_slice(chunk);
        words[index] = u64::from_ne_bytes(raw);
    }

    Some(UserContext {
        general: aether_frame::process::GeneralRegs {
            r15: words[0],
            r14: words[1],
            r13: words[2],
            r12: words[3],
            r11: words[4],
            r10: words[5],
            r9: words[6],
            r8: words[7],
            rdi: words[8],
            rsi: words[9],
            rbp: words[10],
            rbx: words[11],
            rdx: words[12],
            rcx: words[13],
            rax: words[14],
            rip: words[15],
            rsp: words[16],
            rflags: words[17],
        },
        trap_num: 0,
        error_code: 0,
        fs_base: words[18],
        gs_base: words[19],
    })
}

fn serialize_siginfo(info: SignalInfo) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(16);
    bytes.extend_from_slice(&(info.signal as i32).to_ne_bytes());
    bytes.extend_from_slice(&info.code.to_ne_bytes());
    bytes.extend_from_slice(&info.status.to_ne_bytes());
    bytes.extend_from_slice(&0i32.to_ne_bytes());
    bytes
}

fn serialize_ucontext(context: UserContext, mask: u64) -> Vec<u8> {
    let mut bytes = serialize_context(context);
    bytes.extend_from_slice(&mask.to_ne_bytes());
    bytes
}

fn write_user(
    address_space: &mut aether_process::UserAddressSpace,
    address: u64,
    bytes: &[u8],
) -> Result<(), SignalFrameError> {
    let written = address_space
        .write(address, bytes)
        .map_err(|_| SignalFrameError::UserMemory)?;
    if written != bytes.len() {
        return Err(SignalFrameError::UserMemory);
    }
    Ok(())
}

fn align_down(value: u64, align: u64) -> u64 {
    value & !(align - 1)
}

fn align_signal_stack(value: u64) -> u64 {
    align_down(value, X86_64_SIGNAL_FRAME_ALIGN).saturating_sub(8)
}
