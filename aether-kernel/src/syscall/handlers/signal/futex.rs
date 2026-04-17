use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
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
                Ok(self.services.wake_futex(uaddr, bitset, val as usize) as u64)
            }
            FUTEX_REQUEUE => Ok(self.services.requeue_futex(
                uaddr,
                uaddr2,
                val as usize,
                usize::MAX,
                FUTEX_BITSET_MATCH_ANY,
            ) as u64),
            FUTEX_CMP_REQUEUE => {
                if self.read_futex_word(uaddr)? != val3 as u32 {
                    return Err(SysErr::Again);
                }
                Ok(self.services.requeue_futex(
                    uaddr,
                    uaddr2,
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
        self.resumable_blocking_syscall(
            |_ctx, result| match result {
                BlockResult::Futex { woke: true } => SyscallDisposition::ok(0),
                BlockResult::SignalInterrupted => SyscallDisposition::err(SysErr::Intr),
                _ => SyscallDisposition::err(SysErr::Intr),
            },
            |ctx| ctx.syscall_futex(uaddr, operation, val, timeout, uaddr2, val3),
            |ctx| ctx.block_futex(uaddr, bitset),
        )
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
}
