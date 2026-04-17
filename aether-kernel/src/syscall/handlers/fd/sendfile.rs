use alloc::vec;

use aether_vfs::{FsError, NodeKind, PollEvents};

use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::{KernelSyscallContext, SyscallDisposition};

const SENDFILE_CHUNK_SIZE: usize = 64 * 1024;
const MAX_RW_COUNT: usize = 0x7fff_f000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SendfileBlock {
    Read,
    Write,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SendfileStep {
    Complete(u64),
    WouldBlock(SendfileBlock),
}

crate::declare_syscall!(
    pub struct SendfileSyscall => nr::SENDFILE, "sendfile", |ctx, args| {
        ctx.sendfile_blocking(args.get(0), args.get(1), args.get(2), args.get(3) as usize)
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn syscall_sendfile(
        &mut self,
        out_fd: u64,
        in_fd: u64,
        offset: u64,
        count: usize,
    ) -> SysResult<u64> {
        match self.sendfile_step(out_fd, in_fd, offset, count)? {
            SendfileStep::Complete(value) => Ok(value),
            SendfileStep::WouldBlock(_) => Err(SysErr::Again),
        }
    }

    pub(crate) fn syscall_sendfile_blocking(
        &mut self,
        out_fd: u64,
        in_fd: u64,
        offset: u64,
        count: usize,
    ) -> SyscallDisposition {
        self.restartable_blocking_syscall(
            |ctx| match ctx.sendfile_step(out_fd, in_fd, offset, count)? {
                SendfileStep::Complete(value) => Ok(value),
                SendfileStep::WouldBlock(_) => Err(SysErr::Again),
            },
            |ctx| match ctx.sendfile_step(out_fd, in_fd, offset, count) {
                Ok(SendfileStep::WouldBlock(SendfileBlock::Read)) => {
                    ctx.block_file(in_fd as u32, PollEvents::READ)
                }
                Ok(SendfileStep::WouldBlock(SendfileBlock::Write)) => {
                    ctx.block_file(out_fd as u32, PollEvents::WRITE)
                }
                Ok(SendfileStep::Complete(value)) => SyscallDisposition::ok(value),
                Err(error) => SyscallDisposition::err(error),
            },
        )
    }

    fn sendfile_step(
        &mut self,
        out_fd: u64,
        in_fd: u64,
        offset_ptr: u64,
        count: usize,
    ) -> SysResult<SendfileStep> {
        if out_fd == in_fd {
            return Err(SysErr::Inval);
        }
        if count == 0 {
            return Ok(SendfileStep::Complete(0));
        }

        let out_descriptor = self.process.files.get(out_fd as u32).ok_or(SysErr::BadFd)?;
        let in_descriptor = self.process.files.get(in_fd as u32).ok_or(SysErr::BadFd)?;
        let out_file_ref = out_descriptor.file.clone();
        let in_file_ref = in_descriptor.file.clone();

        let out_file = out_file_ref.lock();
        let out_nonblock = out_file.flags().nonblock();
        if out_file.flags().append() {
            return Err(SysErr::Inval);
        }
        let out_node_kind = out_file.node().kind();
        drop(out_file);

        let in_file = in_file_ref.lock();
        let in_nonblock = in_file.flags().nonblock();
        let in_node = in_file.node();
        let in_node_kind = in_node.kind();
        drop(in_file);

        match out_node_kind {
            NodeKind::Directory => return Err(SysErr::IsDir),
            NodeKind::Fifo => return Err(SysErr::SPipe),
            NodeKind::File
            | NodeKind::Socket
            | NodeKind::Symlink
            | NodeKind::BlockDevice
            | NodeKind::CharDevice => {}
        }

        match in_node_kind {
            NodeKind::File | NodeKind::BlockDevice => {}
            _ => return Err(SysErr::Inval),
        }

        let mut total = 0usize;
        let mut remaining = core::cmp::min(count, MAX_RW_COUNT);
        let mut chunk = vec![0u8; core::cmp::min(remaining, SENDFILE_CHUNK_SIZE)];
        let mut explicit_offset = if offset_ptr != 0 {
            let raw = self.syscall_read_user_exact_buffer(offset_ptr, 8)?;
            let value = i64::from_ne_bytes(raw[..8].try_into().map_err(|_| SysErr::Fault)?);
            if value < 0 {
                return Err(SysErr::Inval);
            }
            Some(value as usize)
        } else {
            None
        };

        while remaining != 0 {
            let wanted = core::cmp::min(remaining, chunk.len());
            let read = if let Some(position) = explicit_offset {
                match in_node.read(position, &mut chunk[..wanted]) {
                    Ok(read) => read,
                    Err(FsError::WouldBlock) if total != 0 || in_nonblock => break,
                    Err(FsError::WouldBlock) => {
                        return Ok(SendfileStep::WouldBlock(SendfileBlock::Read));
                    }
                    Err(_) if total != 0 => break,
                    Err(error) => return Err(SysErr::from(error)),
                }
            } else {
                match in_file_ref.lock().read(&mut chunk[..wanted]) {
                    Ok(read) => read,
                    Err(FsError::WouldBlock) if total != 0 || in_nonblock => break,
                    Err(FsError::WouldBlock) => {
                        return Ok(SendfileStep::WouldBlock(SendfileBlock::Read));
                    }
                    Err(_) if total != 0 => break,
                    Err(error) => return Err(SysErr::from(error)),
                }
            };

            if read == 0 {
                break;
            }

            let written = match out_file_ref.lock().write(&chunk[..read]) {
                Ok(written) => written,
                Err(FsError::WouldBlock) if total != 0 || out_nonblock => break,
                Err(FsError::WouldBlock) => {
                    return Ok(SendfileStep::WouldBlock(SendfileBlock::Write));
                }
                Err(_) if total != 0 => break,
                Err(error) => return Err(SysErr::from(error)),
            };

            total = total.saturating_add(written);
            remaining -= written;
            if let Some(position) = explicit_offset.as_mut() {
                *position = position.saturating_add(written);
            }

            if written < read {
                break;
            }
        }

        if let Some(position) = explicit_offset
            && total != 0
        {
            self.write_user_buffer(offset_ptr, &(position as i64).to_ne_bytes())?;
        }

        Ok(SendfileStep::Complete(total as u64))
    }
}
