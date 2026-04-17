#![allow(dead_code)]

use alloc::string::String;

use crate::errno::{SysErr, SysResult};
use crate::syscall::KernelSyscallContext;

pub const AT_FDCWD: i64 = -100;
pub const AT_SYMLINK_NOFOLLOW: u64 = 0x100;
pub const AT_REMOVEDIR: u64 = 0x200;
pub const AT_EACCESS: u64 = 0x200;
pub const AT_STATX_FORCE_SYNC: u64 = 0x2000;
pub const AT_STATX_DONT_SYNC: u64 = 0x4000;
pub const AT_EMPTY_PATH: u64 = 0x1000;
pub const AT_STATX_SYNC_TYPE: u64 = AT_STATX_FORCE_SYNC | AT_STATX_DONT_SYNC;

pub const TIMER_ABSTIME: u64 = 1;
pub const CLOCK_REALTIME: u64 = 0;
pub const CLOCK_MONOTONIC: u64 = 1;
pub const CLOCK_PROCESS_CPUTIME_ID: u64 = 2;
pub const CLOCK_THREAD_CPUTIME_ID: u64 = 3;
pub const CLOCK_MONOTONIC_RAW: u64 = 4;
pub const CLOCK_REALTIME_COARSE: u64 = 5;
pub const CLOCK_MONOTONIC_COARSE: u64 = 6;
pub const CLOCK_BOOTTIME: u64 = 7;
pub const CLOCK_REALTIME_ALARM: u64 = 8;
pub const CLOCK_BOOTTIME_ALARM: u64 = 9;
pub const CLOCK_TAI: u64 = 11;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinuxTimespec {
    pub tv_sec: i64,
    pub tv_nsec: i64,
}

impl LinuxTimespec {
    pub const SIZE: usize = 16;

    pub fn read_from(ctx: &dyn KernelSyscallContext, address: u64) -> SysResult<Self> {
        let bytes = ctx.read_user_buffer(address, Self::SIZE)?;
        if bytes.len() != Self::SIZE {
            return Err(SysErr::Fault);
        }

        Ok(Self {
            tv_sec: i64::from_ne_bytes(bytes[..8].try_into().map_err(|_| SysErr::Fault)?),
            tv_nsec: i64::from_ne_bytes(bytes[8..].try_into().map_err(|_| SysErr::Fault)?),
        })
    }

    pub fn validate(self) -> SysResult<Self> {
        if self.tv_sec < 0 || !(0..1_000_000_000).contains(&self.tv_nsec) {
            return Err(SysErr::Inval);
        }
        Ok(self)
    }

    pub fn total_nanos(self) -> SysResult<u64> {
        self.validate()?;
        Ok((self.tv_sec as u64)
            .saturating_mul(1_000_000_000)
            .saturating_add(self.tv_nsec as u64))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinuxRlimit {
    pub rlim_cur: u64,
    pub rlim_max: u64,
}

impl LinuxRlimit {
    pub const SIZE: usize = 16;

    pub fn read_from(ctx: &dyn KernelSyscallContext, address: u64) -> SysResult<Self> {
        let bytes = ctx.read_user_buffer(address, Self::SIZE)?;
        if bytes.len() != Self::SIZE {
            return Err(SysErr::Fault);
        }

        Ok(Self {
            rlim_cur: u64::from_ne_bytes(bytes[..8].try_into().map_err(|_| SysErr::Fault)?),
            rlim_max: u64::from_ne_bytes(bytes[8..].try_into().map_err(|_| SysErr::Fault)?),
        })
    }

    pub fn write_to(self, ctx: &mut dyn KernelSyscallContext, address: u64) -> SysResult<()> {
        let mut bytes = [0u8; Self::SIZE];
        bytes[..8].copy_from_slice(&self.rlim_cur.to_ne_bytes());
        bytes[8..].copy_from_slice(&self.rlim_max.to_ne_bytes());
        ctx.write_user_buffer(address, &bytes)
    }

    pub fn validate(self) -> SysResult<Self> {
        if self.rlim_cur > self.rlim_max {
            return Err(SysErr::Inval);
        }
        Ok(self)
    }
}

pub fn read_path(ctx: &dyn KernelSyscallContext, pointer: u64, limit: usize) -> SysResult<String> {
    ctx.read_user_c_string(pointer, limit)
}

pub fn read_path_allow_empty(
    ctx: &dyn KernelSyscallContext,
    pointer: u64,
    flags: u64,
    limit: usize,
) -> SysResult<String> {
    if pointer == 0 {
        if (flags & AT_EMPTY_PATH) != 0 {
            return Ok(String::new());
        }
        return Err(SysErr::Fault);
    }
    ctx.read_user_c_string(pointer, limit)
}

pub fn read_optional_string(
    ctx: &dyn KernelSyscallContext,
    pointer: u64,
    limit: usize,
) -> SysResult<Option<String>> {
    if pointer == 0 {
        Ok(None)
    } else {
        ctx.read_user_c_string(pointer, limit).map(Some)
    }
}

pub fn join_u64_halves(low: u64, high: u64) -> u64 {
    low | (high << 32)
}

pub const fn arg_i32(raw: u64) -> i32 {
    raw as u32 as i32
}

pub const fn arg_i64_from_i32(raw: u64) -> i64 {
    arg_i32(raw) as i64
}

pub fn validate_clock_nanosleep_clock(clock_id: u64) -> SysResult<()> {
    if clock_id == CLOCK_REALTIME || clock_id == CLOCK_MONOTONIC {
        Ok(())
    } else {
        Err(SysErr::Inval)
    }
}
