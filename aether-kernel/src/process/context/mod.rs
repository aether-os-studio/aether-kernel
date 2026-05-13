pub(crate) mod fd;
mod process;
mod user;

use alloc::vec;
use alloc::vec::Vec;

use aether_vfs::NodeRef;
use aether_vfs::{NodeKind, PollEvents};

use super::{KernelProcess, ProcessServices};
use crate::errno::SysErr;
use crate::errno::SysResult;
use crate::fs::{FileSystemIdentity, linux_open_flags};
use crate::process::FutexKey;
use crate::rootfs::FsLocation;
use crate::syscall::{BlockResult, BlockType, SyscallDisposition};

pub(crate) struct ProcessSyscallContext<'a> {
    pub(crate) process: &'a mut KernelProcess,
    pub(crate) services: &'a mut dyn ProcessServices,
}

impl ProcessSyscallContext<'_> {
    pub(crate) fn pid(&self) -> u32 {
        self.process.identity.thread_group
    }

    pub(crate) fn masked_mode(&self, mode: u64, file_type: u32) -> u32 {
        file_type | ((mode as u32) & !u32::from(self.process.umask) & 0o7777)
    }

    pub(crate) fn open_node(
        &mut self,
        node: NodeRef,
        filesystem: FileSystemIdentity,
        location: Option<FsLocation>,
        flags: u64,
    ) -> SysResult<u64> {
        const O_CLOEXEC: u64 = 0o2000000;
        const O_DIRECTORY: u64 = 0o200000;
        const O_TRUNC: u64 = 0o1000;

        if (flags & O_DIRECTORY) != 0 && node.kind() != NodeKind::Directory {
            return Err(SysErr::NotDir);
        }

        if (flags & O_TRUNC) != 0 && node.kind() == NodeKind::File {
            node.truncate(0).map_err(SysErr::from)?;
        }

        Ok(self.process.files.insert_node(
            node,
            linux_open_flags(flags),
            filesystem,
            location,
            (flags & O_CLOEXEC) != 0,
        ) as u64)
    }

    pub(crate) fn dup_to(
        &mut self,
        oldfd: u64,
        newfd: u64,
        flags: u64,
        reject_same_fd: bool,
    ) -> SysResult<u64> {
        const O_CLOEXEC: u64 = 0o2000000;

        if oldfd == newfd {
            return if reject_same_fd || flags != 0 {
                Err(SysErr::Inval)
            } else {
                self.process
                    .files
                    .get(oldfd as u32)
                    .map(|_| newfd)
                    .ok_or(SysErr::BadFd)
            };
        }

        if flags & !O_CLOEXEC != 0 {
            return Err(SysErr::Inval);
        }

        self.process
            .files
            .duplicate_to(oldfd as u32, newfd as u32, (flags & O_CLOEXEC) != 0)
            .map(u64::from)
            .ok_or(SysErr::BadFd)
    }

    fn take_wake_result_or_block(
        &mut self,
        block: BlockType,
    ) -> Result<BlockResult, SyscallDisposition> {
        if let Some(result) = self.process.wake_result.take() {
            return Ok(result);
        }
        Ok(self.wait_with_kernel_continuation(block))
    }

    pub(crate) fn wait_file(
        &mut self,
        fd: u32,
        events: PollEvents,
    ) -> Result<BlockResult, SyscallDisposition> {
        self.take_wake_result_or_block(BlockType::File { fd, events })
    }

    pub(crate) fn wait_poll(
        &mut self,
        deadline_nanos: Option<u64>,
        registrations: &[crate::process::PendingPollRegistration],
    ) -> Result<BlockResult, SyscallDisposition> {
        if let Some(result) = self.process.wake_result.take() {
            self.process.pending_file_waits.clear();
            return Ok(result);
        }
        self.process.pending_file_waits.clear();
        self.process
            .pending_file_waits
            .extend_from_slice(registrations);
        let result = self.wait_with_kernel_continuation(BlockType::Poll { deadline_nanos });
        self.process.pending_file_waits.clear();
        Ok(result)
    }

    pub(crate) fn wait_timer(
        &mut self,
        target_nanos: u64,
        request_nanos: u64,
        rmtp: u64,
        flags: u64,
    ) -> Result<BlockResult, SyscallDisposition> {
        self.take_wake_result_or_block(BlockType::Timer {
            target_nanos,
            request_nanos,
            rmtp,
            flags,
        })
    }

    pub(crate) fn wait_futex(
        &mut self,
        key: FutexKey,
        bitset: u32,
        deadline_nanos: Option<u64>,
    ) -> Result<BlockResult, SyscallDisposition> {
        self.take_wake_result_or_block(BlockType::Futex {
            key,
            bitset,
            deadline_nanos,
        })
    }

    pub(crate) fn wait_wait_child(
        &mut self,
        selector: crate::process::WaitChildSelector,
        api: crate::process::WaitChildApi,
        status_ptr: u64,
        info_ptr: u64,
        options: u64,
    ) -> Result<BlockResult, SyscallDisposition> {
        self.take_wake_result_or_block(BlockType::WaitChild {
            selector,
            api,
            status_ptr,
            info_ptr,
            options,
        })
    }

    pub(crate) fn wait_vfork(&mut self, child: u32) -> Result<BlockResult, SyscallDisposition> {
        self.take_wake_result_or_block(BlockType::Vfork { child })
    }

    pub(crate) fn wait_signal_suspend(&mut self) -> Result<BlockResult, SyscallDisposition> {
        self.take_wake_result_or_block(BlockType::SignalSuspend)
    }

    pub(crate) fn wait_with_kernel_continuation(&mut self, block: BlockType) -> BlockResult {
        if let Some(result) = self.process.wake_result.take() {
            return result;
        }

        let _ = self
            .process
            .pending_syscall
            .expect("kernel continuation wait requires a pending syscall");
        self.process.pending_block = Some(block);
        let context = self
            .process
            .kernel_context
            .as_mut()
            .expect("kernel continuation wait requires an active kernel context");
        aether_frame::process::switch_to_scheduler(context);

        self.process
            .wake_result
            .take()
            .unwrap_or(BlockResult::SignalInterrupted)
    }

    pub(crate) fn file_blocking_syscall<F>(
        &mut self,
        fd: u32,
        events: PollEvents,
        mut syscall: F,
    ) -> SyscallDisposition
    where
        F: FnMut(&mut Self) -> SysResult<u64>,
    {
        let nonblock = self
            .process
            .files
            .get(fd)
            .map(|descriptor| descriptor.file.lock().flags().nonblock())
            .unwrap_or(false);
        if nonblock {
            return match syscall(self) {
                Ok(value) => SyscallDisposition::ok(value),
                Err(error) => SyscallDisposition::err(error),
            };
        }

        loop {
            match syscall(self) {
                Ok(value) => return SyscallDisposition::ok(value),
                Err(SysErr::Again) => match self.wait_file(fd, events) {
                    Ok(BlockResult::File { ready: true }) => {}
                    Ok(BlockResult::SignalInterrupted) => {
                        return SyscallDisposition::err(SysErr::Intr);
                    }
                    Ok(_) => return SyscallDisposition::err(SysErr::Intr),
                    Err(disposition) => return disposition,
                },
                Err(error) => return SyscallDisposition::err(error),
            }
        }
    }
}
