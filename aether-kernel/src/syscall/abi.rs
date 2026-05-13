use alloc::string::String;

use crate::errno::{SysErr, SysResult};
use crate::process::ProcessSyscallContext;

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
pub const ITIMER_REAL: u64 = 0;
pub const ITIMER_VIRTUAL: u64 = 1;
pub const ITIMER_PROF: u64 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinuxTimespec {
    pub tv_sec: i64,
    pub tv_nsec: i64,
}

impl LinuxTimespec {
    pub const SIZE: usize = 16;

    pub(crate) fn read_from(ctx: &ProcessSyscallContext<'_>, address: u64) -> SysResult<Self> {
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
pub struct LinuxTimeval {
    pub tv_sec: i64,
    pub tv_usec: i64,
}

impl LinuxTimeval {
    pub const SIZE: usize = 16;

    pub fn validate(self) -> SysResult<Self> {
        if self.tv_sec < 0 || !(0..1_000_000).contains(&self.tv_usec) {
            return Err(SysErr::Inval);
        }
        Ok(self)
    }

    pub fn total_nanos(self) -> SysResult<u64> {
        self.validate()?;
        Ok((self.tv_sec as u64)
            .saturating_mul(1_000_000_000)
            .saturating_add((self.tv_usec as u64).saturating_mul(1_000)))
    }

    pub fn from_nanos(nanos: u64) -> Self {
        Self {
            tv_sec: (nanos / 1_000_000_000) as i64,
            tv_usec: ((nanos % 1_000_000_000) / 1_000) as i64,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinuxItimerval {
    pub it_interval: LinuxTimeval,
    pub it_value: LinuxTimeval,
}

impl LinuxItimerval {
    pub const SIZE: usize = LinuxTimeval::SIZE * 2;

    pub(crate) fn read_from(ctx: &ProcessSyscallContext<'_>, address: u64) -> SysResult<Self> {
        let bytes = ctx.read_user_buffer(address, Self::SIZE)?;
        if bytes.len() != Self::SIZE {
            return Err(SysErr::Fault);
        }

        Ok(Self {
            it_interval: LinuxTimeval {
                tv_sec: i64::from_ne_bytes(bytes[0..8].try_into().map_err(|_| SysErr::Fault)?),
                tv_usec: i64::from_ne_bytes(bytes[8..16].try_into().map_err(|_| SysErr::Fault)?),
            },
            it_value: LinuxTimeval {
                tv_sec: i64::from_ne_bytes(bytes[16..24].try_into().map_err(|_| SysErr::Fault)?),
                tv_usec: i64::from_ne_bytes(bytes[24..32].try_into().map_err(|_| SysErr::Fault)?),
            },
        })
    }

    pub(crate) fn write_to(
        self,
        ctx: &mut ProcessSyscallContext<'_>,
        address: u64,
    ) -> SysResult<()> {
        let mut bytes = [0u8; Self::SIZE];
        bytes[0..8].copy_from_slice(&self.it_interval.tv_sec.to_ne_bytes());
        bytes[8..16].copy_from_slice(&self.it_interval.tv_usec.to_ne_bytes());
        bytes[16..24].copy_from_slice(&self.it_value.tv_sec.to_ne_bytes());
        bytes[24..32].copy_from_slice(&self.it_value.tv_usec.to_ne_bytes());
        ctx.write_user_buffer(address, &bytes)
    }

    pub fn from_nanos(interval_nanos: u64, value_nanos: u64) -> Self {
        Self {
            it_interval: LinuxTimeval::from_nanos(interval_nanos),
            it_value: LinuxTimeval::from_nanos(value_nanos),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinuxRlimit {
    pub rlim_cur: u64,
    pub rlim_max: u64,
}

impl LinuxRlimit {
    pub const SIZE: usize = 16;

    pub fn validate(self) -> SysResult<Self> {
        if self.rlim_cur > self.rlim_max {
            return Err(SysErr::Inval);
        }
        Ok(self)
    }
}

pub(crate) fn read_path(
    ctx: &ProcessSyscallContext<'_>,
    pointer: u64,
    limit: usize,
) -> SysResult<String> {
    ctx.read_user_c_string(pointer, limit)
}

pub(crate) fn read_path_allow_empty(
    ctx: &ProcessSyscallContext<'_>,
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
