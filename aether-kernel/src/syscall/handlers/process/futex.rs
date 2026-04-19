use aether_frame::time;

use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{FutexKey, ProcessServices, ProcessSyscallContext};
use crate::syscall::abi::LinuxTimespec;
use crate::syscall::{BlockResult, SyscallDisposition};

crate::declare_syscall!(
    pub struct FutexSyscall => nr::FUTEX, "futex", |ctx, args| {
        ctx.futex_blocking(
            args.get(0),
            args.get(1),
            args.get(2),
            args.get(3),
            args.get(4),
            args.get(5),
        )
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_futex(
        &mut self,
        uaddr: u64,
        operation: u64,
        val: u64,
        timeout: u64,
        uaddr2: u64,
        val3: u64,
    ) -> SysResult<u64> {
        const FUTEX_WAIT: u64 = 0;
        const FUTEX_WAKE: u64 = 1;
        const FUTEX_REQUEUE: u64 = 3;
        const FUTEX_CMP_REQUEUE: u64 = 4;
        const FUTEX_WAIT_BITSET: u64 = 9;
        const FUTEX_WAKE_BITSET: u64 = 10;
        const FUTEX_PRIVATE_FLAG: u64 = 128;
        const FUTEX_CLOCK_REALTIME: u64 = 256;
        const FUTEX_CMD_MASK: u64 = !(FUTEX_PRIVATE_FLAG | FUTEX_CLOCK_REALTIME);
        const FUTEX_BITSET_MATCH_ANY: u32 = u32::MAX;

        let command = operation & FUTEX_CMD_MASK;
        let private = (operation & FUTEX_PRIVATE_FLAG) != 0;
        let key = self.futex_key(uaddr, private);
        match command {
            FUTEX_WAIT | FUTEX_WAIT_BITSET => {
                let expected = val as u32;
                let bitset = if command == FUTEX_WAIT_BITSET {
                    if val3 as u32 == 0 {
                        return Err(SysErr::Inval);
                    }
                    val3 as u32
                } else {
                    FUTEX_BITSET_MATCH_ANY
                };
                let current = self.read_futex_word(uaddr)?;
                if current != expected {
                    return Err(SysErr::Again);
                }
                let _ = bitset;
                Err(SysErr::Again)
            }
            FUTEX_WAKE | FUTEX_WAKE_BITSET => {
                let bitset = if command == FUTEX_WAKE_BITSET {
                    if val3 as u32 == 0 {
                        return Err(SysErr::Inval);
                    }
                    val3 as u32
                } else {
                    FUTEX_BITSET_MATCH_ANY
                };
                Ok(self.services.wake_futex(key, bitset, val as usize) as u64)
            }
            FUTEX_REQUEUE => Ok(self.services.requeue_futex(
                key,
                self.futex_key(uaddr2, private),
                val as usize,
                usize::MAX,
                FUTEX_BITSET_MATCH_ANY,
            ) as u64),
            FUTEX_CMP_REQUEUE => {
                if self.read_futex_word(uaddr)? != val3 as u32 {
                    return Err(SysErr::Again);
                }
                Ok(self.services.requeue_futex(
                    key,
                    self.futex_key(uaddr2, private),
                    val as usize,
                    timeout as usize,
                    FUTEX_BITSET_MATCH_ANY,
                ) as u64)
            }
            _ => Err(SysErr::NoSys),
        }
    }

    pub(crate) fn syscall_futex_blocking(
        &mut self,
        uaddr: u64,
        operation: u64,
        val: u64,
        timeout: u64,
        uaddr2: u64,
        val3: u64,
    ) -> SyscallDisposition {
        const FUTEX_WAIT: u64 = 0;
        const FUTEX_WAIT_BITSET: u64 = 9;
        const FUTEX_PRIVATE_FLAG: u64 = 128;
        const FUTEX_CLOCK_REALTIME: u64 = 256;
        const FUTEX_CMD_MASK: u64 = !(FUTEX_PRIVATE_FLAG | FUTEX_CLOCK_REALTIME);
        const FUTEX_BITSET_MATCH_ANY: u32 = u32::MAX;

        let command = operation & FUTEX_CMD_MASK;
        let private = (operation & FUTEX_PRIVATE_FLAG) != 0;
        let key = self.futex_key(uaddr, private);
        if let Some(result) = self.process.wake_result.take() {
            return match result {
                BlockResult::Futex { woke: true, .. } => SyscallDisposition::ok(0),
                BlockResult::Futex {
                    timed_out: true, ..
                } => SyscallDisposition::err(SysErr::TimedOut),
                BlockResult::SignalInterrupted => SyscallDisposition::err(SysErr::Intr),
                _ => SyscallDisposition::err(SysErr::Intr),
            };
        }

        if command != FUTEX_WAIT && command != FUTEX_WAIT_BITSET {
            return SyscallDisposition::Return(
                self.syscall_futex(uaddr, operation, val, timeout, uaddr2, val3),
            );
        }
        let bitset = if command == FUTEX_WAIT_BITSET {
            if val3 as u32 == 0 {
                return SyscallDisposition::err(SysErr::Inval);
            }
            val3 as u32
        } else {
            FUTEX_BITSET_MATCH_ANY
        };
        let deadline_nanos = match self.futex_wait_deadline(command, operation, timeout) {
            Ok(deadline_nanos) => deadline_nanos,
            Err(error) => return SyscallDisposition::err(error),
        };
        let _ = uaddr2;

        let pid = self.process.identity.pid;
        self.services.arm_futex_wait(pid, key, bitset);
        match self.read_futex_word(uaddr) {
            Ok(current) if current != val as u32 => {
                self.services.disarm_futex_wait(pid);
                return SyscallDisposition::err(SysErr::Again);
            }
            Ok(_) => {}
            Err(error) => {
                self.services.disarm_futex_wait(pid);
                return SyscallDisposition::err(error);
            }
        }

        match self.wait_futex(key, bitset, deadline_nanos) {
            Ok(BlockResult::Futex { woke: true, .. }) => SyscallDisposition::ok(0),
            Ok(BlockResult::Futex {
                timed_out: true, ..
            }) => SyscallDisposition::err(SysErr::TimedOut),
            Ok(BlockResult::SignalInterrupted) => SyscallDisposition::err(SysErr::Intr),
            Ok(_) => SyscallDisposition::err(SysErr::Intr),
            Err(disposition) => disposition,
        }
    }

    fn futex_key(&self, uaddr: u64, private: bool) -> FutexKey {
        if private {
            FutexKey::private(self.process.task.address_space.identity(), uaddr)
        } else {
            FutexKey::shared(uaddr)
        }
    }

    fn read_futex_word(&self, uaddr: u64) -> SysResult<u32> {
        let bytes = self
            .process
            .task
            .address_space
            .read_user_exact(uaddr, core::mem::size_of::<u32>())
            .map_err(|_| SysErr::Fault)?;
        Ok(u32::from_ne_bytes(
            bytes.as_slice().try_into().map_err(|_| SysErr::Fault)?,
        ))
    }

    fn futex_wait_deadline(
        &self,
        command: u64,
        operation: u64,
        timeout: u64,
    ) -> SysResult<Option<u64>> {
        const FUTEX_WAIT: u64 = 0;
        const FUTEX_WAIT_BITSET: u64 = 9;
        const FUTEX_CLOCK_REALTIME: u64 = 256;

        if timeout == 0 {
            return Ok(None);
        }

        let request = LinuxTimespec::read_from(self, timeout)?.validate()?;
        let request_nanos = request.total_nanos()?;
        let current_nanos = time::monotonic_nanos();

        match command {
            FUTEX_WAIT => {
                if request_nanos == 0 {
                    return Err(SysErr::TimedOut);
                }
                Ok(Some(current_nanos.saturating_add(request_nanos)))
            }
            FUTEX_WAIT_BITSET => {
                if (operation & FUTEX_CLOCK_REALTIME) != 0 {
                    let current_realtime_nanos = time::realtime_now().total_nanos().unwrap_or(0);
                    if request_nanos <= current_realtime_nanos {
                        return Err(SysErr::TimedOut);
                    }
                    Ok(Some(current_nanos.saturating_add(
                        request_nanos.saturating_sub(current_realtime_nanos),
                    )))
                } else {
                    if request_nanos <= current_nanos {
                        return Err(SysErr::TimedOut);
                    }
                    Ok(Some(request_nanos))
                }
            }
            _ => Ok(None),
        }
    }
}
