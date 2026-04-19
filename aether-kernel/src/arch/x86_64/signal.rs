extern crate alloc;

use alloc::vec::Vec;
use core::mem::size_of;

use aether_frame::process::UserContext;

use crate::errno::{SysErr, SysResult};
use crate::process::KernelProcess;
use crate::signal::{SignalAction, SignalInfo};

const SA_NODEFER: u64 = 0x4000_0000;
const SA_RESTORER: u64 = 0x0400_0000;
const SA_SIGINFO: u64 = 0x0000_0004;
const X86_64_RED_ZONE_SIZE: u64 = 128;
const X86_64_SIGNAL_FRAME_ALIGN: u64 = 16;
const USER_CODE_SELECTOR: u64 = 0x23;
const USER_DATA_SELECTOR: u64 = 0x1b;
const TRAMPOLINE_BYTES: [u8; 9] = [0xb8, 15, 0, 0, 0, 0x0f, 0x05, 0x0f, 0x0b];

const X64_REG_R8: usize = 0;
const X64_REG_R9: usize = 1;
const X64_REG_R10: usize = 2;
const X64_REG_R11: usize = 3;
const X64_REG_R12: usize = 4;
const X64_REG_R13: usize = 5;
const X64_REG_R14: usize = 6;
const X64_REG_R15: usize = 7;
const X64_REG_RDI: usize = 8;
const X64_REG_RSI: usize = 9;
const X64_REG_RBP: usize = 10;
const X64_REG_RBX: usize = 11;
const X64_REG_RDX: usize = 12;
const X64_REG_RAX: usize = 13;
const X64_REG_RCX: usize = 14;
const X64_REG_RSP: usize = 15;
const X64_REG_RIP: usize = 16;
const X64_REG_EFL: usize = 17;
const X64_REG_CSGSFS: usize = 18;
const X64_REG_ERR: usize = 19;
const X64_REG_TRAPNO: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalFrameError {
    InvalidFrame,
    UserMemory,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct LinuxStackT {
    ss_sp: u64,
    ss_flags: i32,
    _pad: i32,
    ss_size: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct LinuxSigSet {
    bits: [u64; 16],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct LinuxMContext {
    gregs: [u64; 23],
    fpregs: u64,
    reserved1: [u64; 8],
}

impl Default for LinuxMContext {
    fn default() -> Self {
        Self {
            gregs: [0; 23],
            fpregs: 0,
            reserved1: [0; 8],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct LinuxUContext {
    uc_flags: u64,
    uc_link: u64,
    uc_stack: LinuxStackT,
    uc_mcontext: LinuxMContext,
    uc_sigmask: LinuxSigSet,
    fpregs_mem: [u8; 512],
    ssp: [u64; 4],
}

impl Default for LinuxUContext {
    fn default() -> Self {
        Self {
            uc_flags: 0,
            uc_link: 0,
            uc_stack: LinuxStackT::default(),
            uc_mcontext: LinuxMContext::default(),
            uc_sigmask: LinuxSigSet::default(),
            fpregs_mem: [0; 512],
            ssp: [0; 4],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct LinuxSiginfo {
    si_signo: i32,
    si_errno: i32,
    si_code: i32,
    _pad0: i32,
    si_pid: i32,
    si_uid: u32,
    si_status: i32,
    _pad1: i32,
    si_utime: i64,
    si_stime: i64,
    _reserved: [u8; 80],
}

impl Default for LinuxSiginfo {
    fn default() -> Self {
        Self {
            si_signo: 0,
            si_errno: 0,
            si_code: 0,
            _pad0: 0,
            si_pid: 0,
            si_uid: 0,
            si_status: 0,
            _pad1: 0,
            si_utime: 0,
            si_stime: 0,
            _reserved: [0; 80],
        }
    }
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
    let layout = SignalFrameLayout::new(saved.general.rsp, action)?;
    let siginfo = build_siginfo(signal);
    let ucontext = build_ucontext(saved, saved_mask);
    let siginfo_bytes = serialize_siginfo(&siginfo);
    let ucontext_bytes = serialize_ucontext(&ucontext);

    write_user(
        &mut process.task.address_space,
        layout.frame_base,
        &layout.return_address,
    )?;
    write_user_struct(
        &mut process.task.address_space,
        layout.ucontext_addr,
        &ucontext_bytes,
    )?;
    write_user_struct(
        &mut process.task.address_space,
        layout.siginfo_addr,
        &siginfo_bytes,
    )?;
    if layout.uses_stack_trampoline {
        write_user(
            &mut process.task.address_space,
            layout.trampoline_addr,
            &TRAMPOLINE_BYTES,
        )?;
    }

    let context = process.task.process.context_mut();
    context.general.rip = action.handler;
    context.general.rsp = layout.frame_base;
    context.general.rdi = signal.signal as u64;
    context.general.rsi = 0;
    context.general.rdx = 0;
    if (action.flags & SA_SIGINFO) != 0 {
        context.general.rsi = layout.siginfo_addr;
        context.general.rdx = layout.ucontext_addr;
    }

    let mut next_mask = saved_mask | action.mask;
    if (action.flags & SA_NODEFER) == 0 {
        next_mask |= crate::signal::sigbit(signal.signal);
    }
    process.signals.set_blocked_mask(next_mask);
    Ok(())
}

pub fn restore_signal_from_user(process: &mut KernelProcess) -> SysResult<u64> {
    let ucontext_addr = process.task.process.context().general.rsp;
    let bytes = process
        .task
        .address_space
        .read_user_exact(ucontext_addr, size_of::<LinuxUContext>())
        .map_err(|_| SysErr::Fault)?;
    let ucontext = decode_ucontext(&bytes).ok_or(SysErr::Fault)?;
    let current = *process.task.process.context();
    let restored = restore_ucontext(current, &ucontext);
    process.signals.restore_mask(ucontext.uc_sigmask.bits[0]);
    *process.task.process.context_mut() = restored;
    Ok(restored.general.rax)
}

struct SignalFrameLayout {
    frame_base: u64,
    ucontext_addr: u64,
    siginfo_addr: u64,
    trampoline_addr: u64,
    uses_stack_trampoline: bool,
    return_address: [u8; 8],
}

impl SignalFrameLayout {
    fn new(user_rsp: u64, action: SignalAction) -> Result<Self, SignalFrameError> {
        let trampoline_bytes = if (action.flags & SA_RESTORER) == 0 || action.restorer == 0 {
            TRAMPOLINE_BYTES.len() as u64
        } else {
            0
        };
        let fixed_bytes = 8
            + size_of::<LinuxUContext>() as u64
            + size_of::<LinuxSiginfo>() as u64
            + trampoline_bytes;
        let stack_top = user_rsp
            .checked_sub(X86_64_RED_ZONE_SIZE)
            .ok_or(SignalFrameError::InvalidFrame)?;
        let mut frame_base = align_down(
            stack_top
                .checked_sub(fixed_bytes)
                .ok_or(SignalFrameError::InvalidFrame)?,
            X86_64_SIGNAL_FRAME_ALIGN,
        )
        .checked_add(8)
        .ok_or(SignalFrameError::InvalidFrame)?;
        if frame_base > stack_top.saturating_sub(fixed_bytes) {
            frame_base = frame_base.saturating_sub(X86_64_SIGNAL_FRAME_ALIGN);
        }

        let ucontext_addr = frame_base + 8;
        let siginfo_addr = ucontext_addr + size_of::<LinuxUContext>() as u64;
        let trampoline_addr = siginfo_addr + size_of::<LinuxSiginfo>() as u64;
        let uses_stack_trampoline = trampoline_bytes != 0;
        let return_address = if uses_stack_trampoline {
            trampoline_addr.to_ne_bytes()
        } else {
            action.restorer.to_ne_bytes()
        };

        Ok(Self {
            frame_base,
            ucontext_addr,
            siginfo_addr,
            trampoline_addr,
            uses_stack_trampoline,
            return_address,
        })
    }
}

fn build_siginfo(info: SignalInfo) -> LinuxSiginfo {
    LinuxSiginfo {
        si_signo: info.signal as i32,
        si_errno: 0,
        si_code: info.code,
        si_pid: info.pid,
        si_uid: info.uid,
        si_status: info.status,
        ..LinuxSiginfo::default()
    }
}

fn build_ucontext(context: UserContext, mask: u64) -> LinuxUContext {
    let mut ucontext = LinuxUContext::default();
    ucontext.uc_mcontext.gregs[X64_REG_R8] = context.general.r8;
    ucontext.uc_mcontext.gregs[X64_REG_R9] = context.general.r9;
    ucontext.uc_mcontext.gregs[X64_REG_R10] = context.general.r10;
    ucontext.uc_mcontext.gregs[X64_REG_R11] = context.general.r11;
    ucontext.uc_mcontext.gregs[X64_REG_R12] = context.general.r12;
    ucontext.uc_mcontext.gregs[X64_REG_R13] = context.general.r13;
    ucontext.uc_mcontext.gregs[X64_REG_R14] = context.general.r14;
    ucontext.uc_mcontext.gregs[X64_REG_R15] = context.general.r15;
    ucontext.uc_mcontext.gregs[X64_REG_RDI] = context.general.rdi;
    ucontext.uc_mcontext.gregs[X64_REG_RSI] = context.general.rsi;
    ucontext.uc_mcontext.gregs[X64_REG_RBP] = context.general.rbp;
    ucontext.uc_mcontext.gregs[X64_REG_RBX] = context.general.rbx;
    ucontext.uc_mcontext.gregs[X64_REG_RDX] = context.general.rdx;
    ucontext.uc_mcontext.gregs[X64_REG_RAX] = context.general.rax;
    ucontext.uc_mcontext.gregs[X64_REG_RCX] = context.general.rcx;
    ucontext.uc_mcontext.gregs[X64_REG_RSP] = context.general.rsp;
    ucontext.uc_mcontext.gregs[X64_REG_RIP] = context.general.rip;
    ucontext.uc_mcontext.gregs[X64_REG_EFL] = context.general.rflags;
    ucontext.uc_mcontext.gregs[X64_REG_CSGSFS] = USER_CODE_SELECTOR | (USER_DATA_SELECTOR << 48);
    ucontext.uc_mcontext.gregs[X64_REG_ERR] = context.error_code;
    ucontext.uc_mcontext.gregs[X64_REG_TRAPNO] = context.trap_num;
    ucontext.uc_sigmask.bits[0] = mask;
    ucontext
}

fn restore_ucontext(mut current: UserContext, ucontext: &LinuxUContext) -> UserContext {
    current.general.r8 = ucontext.uc_mcontext.gregs[X64_REG_R8];
    current.general.r9 = ucontext.uc_mcontext.gregs[X64_REG_R9];
    current.general.r10 = ucontext.uc_mcontext.gregs[X64_REG_R10];
    current.general.r11 = ucontext.uc_mcontext.gregs[X64_REG_R11];
    current.general.r12 = ucontext.uc_mcontext.gregs[X64_REG_R12];
    current.general.r13 = ucontext.uc_mcontext.gregs[X64_REG_R13];
    current.general.r14 = ucontext.uc_mcontext.gregs[X64_REG_R14];
    current.general.r15 = ucontext.uc_mcontext.gregs[X64_REG_R15];
    current.general.rdi = ucontext.uc_mcontext.gregs[X64_REG_RDI];
    current.general.rsi = ucontext.uc_mcontext.gregs[X64_REG_RSI];
    current.general.rbp = ucontext.uc_mcontext.gregs[X64_REG_RBP];
    current.general.rbx = ucontext.uc_mcontext.gregs[X64_REG_RBX];
    current.general.rdx = ucontext.uc_mcontext.gregs[X64_REG_RDX];
    current.general.rax = ucontext.uc_mcontext.gregs[X64_REG_RAX];
    current.general.rcx = ucontext.uc_mcontext.gregs[X64_REG_RCX];
    current.general.rsp = ucontext.uc_mcontext.gregs[X64_REG_RSP];
    current.general.rip = ucontext.uc_mcontext.gregs[X64_REG_RIP];
    current.general.rflags = ucontext.uc_mcontext.gregs[X64_REG_EFL];
    current.trap_num = ucontext.uc_mcontext.gregs[X64_REG_TRAPNO];
    current.error_code = ucontext.uc_mcontext.gregs[X64_REG_ERR];
    current
}

fn write_user_struct(
    address_space: &mut aether_process::UserAddressSpace,
    address: u64,
    value: &[u8],
) -> Result<(), SignalFrameError> {
    write_user(address_space, address, value)
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

fn serialize_siginfo(info: &LinuxSiginfo) -> [u8; 128] {
    let mut bytes = [0u8; 128];
    bytes[0..4].copy_from_slice(&info.si_signo.to_ne_bytes());
    bytes[4..8].copy_from_slice(&info.si_errno.to_ne_bytes());
    bytes[8..12].copy_from_slice(&info.si_code.to_ne_bytes());
    bytes[12..16].copy_from_slice(&info._pad0.to_ne_bytes());
    bytes[16..20].copy_from_slice(&info.si_pid.to_ne_bytes());
    bytes[20..24].copy_from_slice(&info.si_uid.to_ne_bytes());
    bytes[24..28].copy_from_slice(&info.si_status.to_ne_bytes());
    bytes[28..32].copy_from_slice(&info._pad1.to_ne_bytes());
    bytes[32..40].copy_from_slice(&info.si_utime.to_ne_bytes());
    bytes[40..48].copy_from_slice(&info.si_stime.to_ne_bytes());
    bytes[48..128].copy_from_slice(&info._reserved);
    bytes
}

fn serialize_ucontext(ucontext: &LinuxUContext) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(size_of::<LinuxUContext>());
    bytes.extend_from_slice(&ucontext.uc_flags.to_ne_bytes());
    bytes.extend_from_slice(&ucontext.uc_link.to_ne_bytes());
    bytes.extend_from_slice(&ucontext.uc_stack.ss_sp.to_ne_bytes());
    bytes.extend_from_slice(&ucontext.uc_stack.ss_flags.to_ne_bytes());
    bytes.extend_from_slice(&ucontext.uc_stack._pad.to_ne_bytes());
    bytes.extend_from_slice(&ucontext.uc_stack.ss_size.to_ne_bytes());
    for greg in ucontext.uc_mcontext.gregs {
        bytes.extend_from_slice(&greg.to_ne_bytes());
    }
    bytes.extend_from_slice(&ucontext.uc_mcontext.fpregs.to_ne_bytes());
    for reserved in ucontext.uc_mcontext.reserved1 {
        bytes.extend_from_slice(&reserved.to_ne_bytes());
    }
    for word in ucontext.uc_sigmask.bits {
        bytes.extend_from_slice(&word.to_ne_bytes());
    }
    bytes.extend_from_slice(&ucontext.fpregs_mem);
    for word in ucontext.ssp {
        bytes.extend_from_slice(&word.to_ne_bytes());
    }
    bytes
}

fn decode_ucontext(bytes: &[u8]) -> Option<LinuxUContext> {
    if bytes.len() != size_of::<LinuxUContext>() {
        return None;
    }

    let mut offset = 0usize;
    let uc_flags = read_u64(bytes, &mut offset)?;
    let uc_link = read_u64(bytes, &mut offset)?;
    let ss_sp = read_u64(bytes, &mut offset)?;
    let ss_flags = read_i32(bytes, &mut offset)?;
    let ss_pad = read_i32(bytes, &mut offset)?;
    let ss_size = read_u64(bytes, &mut offset)?;

    let mut gregs = [0u64; 23];
    for greg in &mut gregs {
        *greg = read_u64(bytes, &mut offset)?;
    }
    let fpregs = read_u64(bytes, &mut offset)?;
    let mut reserved1 = [0u64; 8];
    for reserved in &mut reserved1 {
        *reserved = read_u64(bytes, &mut offset)?;
    }
    let mut sigmask = [0u64; 16];
    for word in &mut sigmask {
        *word = read_u64(bytes, &mut offset)?;
    }
    let fpregs_mem = bytes.get(offset..offset + 512)?.try_into().ok()?;
    offset += 512;
    let mut ssp = [0u64; 4];
    for word in &mut ssp {
        *word = read_u64(bytes, &mut offset)?;
    }
    if offset != bytes.len() {
        return None;
    }

    Some(LinuxUContext {
        uc_flags,
        uc_link,
        uc_stack: LinuxStackT {
            ss_sp,
            ss_flags,
            _pad: ss_pad,
            ss_size,
        },
        uc_mcontext: LinuxMContext {
            gregs,
            fpregs,
            reserved1,
        },
        uc_sigmask: LinuxSigSet { bits: sigmask },
        fpregs_mem,
        ssp,
    })
}

fn read_u64(bytes: &[u8], offset: &mut usize) -> Option<u64> {
    let chunk = bytes.get(*offset..*offset + 8)?;
    *offset += 8;
    Some(u64::from_ne_bytes(chunk.try_into().ok()?))
}

fn read_i32(bytes: &[u8], offset: &mut usize) -> Option<i32> {
    let chunk = bytes.get(*offset..*offset + 4)?;
    *offset += 4;
    Some(i32::from_ne_bytes(chunk.try_into().ok()?))
}
