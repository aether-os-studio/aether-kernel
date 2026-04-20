extern crate alloc;

use alloc::vec::Vec;

pub type SigSet = u64;

pub const SIGNAL_MAX: usize = 64;
pub const SIGHUP: u8 = 1;
pub const SIGINT: u8 = 2;
pub const SIGQUIT: u8 = 3;
pub const SIGILL: u8 = 4;
pub const SIGTRAP: u8 = 5;
pub const SIGABRT: u8 = 6;
pub const SIGBUS: u8 = 7;
pub const SIGFPE: u8 = 8;
pub const SIGKILL: u8 = 9;
pub const SIGUSR1: u8 = 10;
pub const SIGSEGV: u8 = 11;
pub const SIGUSR2: u8 = 12;
pub const SIGPIPE: u8 = 13;
pub const SIGALRM: u8 = 14;
pub const SIGTERM: u8 = 15;
pub const SIGCHLD: u8 = 17;
pub const SIGCONT: u8 = 18;
pub const SIGSTOP: u8 = 19;
pub const SIGTSTP: u8 = 20;
pub const SIGTTIN: u8 = 21;
pub const SIGTTOU: u8 = 22;
pub const SIGURG: u8 = 23;
pub const SIGXCPU: u8 = 24;
pub const SIGXFSZ: u8 = 25;
pub const SIGVTALRM: u8 = 26;
pub const SIGPROF: u8 = 27;
pub const SIGWINCH: u8 = 28;
pub const SIGIO: u8 = 29;
pub const SIGPWR: u8 = 30;
pub const SIGSYS: u8 = 31;

pub const SIG_BLOCK: u64 = 0;
pub const SIG_UNBLOCK: u64 = 1;
pub const SIG_SETMASK: u64 = 2;

pub const SIG_DFL: u64 = 0;
pub const SIG_IGN: u64 = 1;

pub const SA_NOCLDSTOP: u64 = 0x0000_0001;
pub const SA_NOCLDWAIT: u64 = 0x0000_0002;

pub const CLD_EXITED: i32 = 1;
pub const CLD_KILLED: i32 = 2;
pub const CLD_DUMPED: i32 = 3;
pub const CLD_STOPPED: i32 = 5;
pub const CLD_CONTINUED: i32 = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignalInfo {
    pub signal: u8,
    pub code: i32,
    pub status: i32,
    pub pid: i32,
    pub uid: u32,
}

impl SignalInfo {
    #[allow(dead_code)]
    pub const fn child_exit(pid: u32, status: i32) -> Self {
        let (code, status) = if status >= 128 {
            // TODO: distinguish `CLD_DUMPED` once the kernel tracks core-dump termination
            // separately from generic signal death.
            (CLD_KILLED, status - 128)
        } else {
            (CLD_EXITED, status)
        };
        Self {
            signal: SIGCHLD,
            code,
            status,
            pid: pid as i32,
            uid: 0,
        }
    }

    pub const fn child_stop(pid: u32, signal: u8) -> Self {
        Self {
            signal: SIGCHLD,
            code: CLD_STOPPED,
            status: signal as i32,
            pid: pid as i32,
            uid: 0,
        }
    }

    pub const fn child_continue(pid: u32) -> Self {
        Self {
            signal: SIGCHLD,
            code: CLD_CONTINUED,
            status: 0,
            pid: pid as i32,
            uid: 0,
        }
    }

    pub const fn kernel(signal: u8, code: i32) -> Self {
        Self {
            signal,
            code,
            status: 0,
            pid: 0,
            uid: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignalAction {
    pub handler: u64,
    pub flags: u64,
    pub restorer: u64,
    pub mask: SigSet,
}

impl SignalAction {
    pub const fn default_for(signal: u8) -> Self {
        let _ = signal;
        Self {
            handler: SIG_DFL,
            flags: 0,
            restorer: 0,
            mask: 0,
        }
    }
}

pub fn sigbit(signal: u8) -> SigSet {
    if signal == 0 || signal as usize > SIGNAL_MAX {
        0
    } else {
        1u64 << (signal - 1)
    }
}

pub fn parse_sigaction(bytes: &[u8]) -> Option<SignalAction> {
    if bytes.len() < 32 {
        return None;
    }

    Some(SignalAction {
        handler: read_u64(bytes, 0)?,
        flags: read_u64(bytes, 8)?,
        restorer: read_u64(bytes, 16)?,
        mask: read_u64(bytes, 24)?,
    })
}

pub fn serialize_sigaction(action: SignalAction) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(32);
    bytes.extend_from_slice(&action.handler.to_ne_bytes());
    bytes.extend_from_slice(&action.flags.to_ne_bytes());
    bytes.extend_from_slice(&action.restorer.to_ne_bytes());
    bytes.extend_from_slice(&action.mask.to_ne_bytes());
    bytes
}

fn read_u64(bytes: &[u8], offset: usize) -> Option<u64> {
    let raw = bytes.get(offset..offset + 8)?;
    let mut value = [0; 8];
    value.copy_from_slice(raw);
    Some(u64::from_ne_bytes(value))
}
