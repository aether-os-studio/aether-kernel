use alloc::vec;

use aether_vfs::{FsError, NodeKind, PollEvents};

use crate::arch::syscall::nr;
use crate::errno::{SysErr, SysResult};
use crate::process::ProcessSyscallContext;
use crate::syscall::SyscallDisposition;

const COPY_FILE_RANGE_CHUNK_SIZE: usize = 64 * 1024;
const MAX_RW_COUNT: usize = 0x7fff_f000;

#[derive(Clone, Copy)]
enum CopyFileRangeBlock {
    Read,
    Write,
}

#[derive(Clone, Copy)]
enum CopyFileRangeStep {
    Complete(u64),
    WouldBlock(CopyFileRangeBlock),
}

crate::declare_syscall!(
    pub struct CopyFileRangeSyscall => nr::COPY_FILE_RANGE, "copy_file_range", |ctx, args| {
        ctx.copy_file_range_blocking(
            args.get(0),
            args.get(1),
            args.get(2),
            args.get(3),
            args.get(4) as usize,
            args.get(5),
        )
    }
);

impl ProcessSyscallContext<'_> {
    pub(crate) fn copy_file_range_blocking(
        &mut self,
        in_fd: u64,
        off_in: u64,
        out_fd: u64,
        off_out: u64,
        len: usize,
        flags: u64,
    ) -> SyscallDisposition {
        loop {
            match self.copy_file_range_step(in_fd, off_in, out_fd, off_out, len, flags) {
                Ok(CopyFileRangeStep::Complete(value)) => return SyscallDisposition::ok(value),
                Ok(CopyFileRangeStep::WouldBlock(CopyFileRangeBlock::Read)) => {
                    match self.wait_file(in_fd as u32, PollEvents::READ) {
                        Ok(crate::syscall::BlockResult::File { ready: true }) => {}
                        Ok(crate::syscall::BlockResult::SignalInterrupted) => {
                            return SyscallDisposition::err(SysErr::Intr);
                        }
                        Ok(_) => return SyscallDisposition::err(SysErr::Intr),
                        Err(disposition) => return disposition,
                    }
                }
                Ok(CopyFileRangeStep::WouldBlock(CopyFileRangeBlock::Write)) => {
                    match self.wait_file(out_fd as u32, PollEvents::WRITE) {
                        Ok(crate::syscall::BlockResult::File { ready: true }) => {}
                        Ok(crate::syscall::BlockResult::SignalInterrupted) => {
                            return SyscallDisposition::err(SysErr::Intr);
                        }
                        Ok(_) => return SyscallDisposition::err(SysErr::Intr),
                        Err(disposition) => return disposition,
                    }
                }
                Err(error) => return SyscallDisposition::err(error),
            }
        }
    }

    fn copy_file_range_step(
        &mut self,
        in_fd: u64,
        off_in_ptr: u64,
        out_fd: u64,
        off_out_ptr: u64,
        len: usize,
        flags: u64,
    ) -> SysResult<CopyFileRangeStep> {
        if flags != 0 {
            return Err(SysErr::Inval);
        }
        if len == 0 {
            return Ok(CopyFileRangeStep::Complete(0));
        }

        let in_descriptor = self.process.files.get(in_fd as u32).ok_or(SysErr::BadFd)?;
        let out_descriptor = self.process.files.get(out_fd as u32).ok_or(SysErr::BadFd)?;
        let in_file_ref = in_descriptor.file.clone();
        let out_file_ref = out_descriptor.file.clone();

        let in_file = in_file_ref.lock();
        let in_nonblock = in_file.flags().nonblock();
        let in_node = in_file.node();
        let in_node_kind = in_node.kind();
        drop(in_file);

        let out_file = out_file_ref.lock();
        let out_nonblock = out_file.flags().nonblock();
        if out_file.flags().append() {
            return Err(SysErr::Inval);
        }
        let out_node_kind = out_file.node().kind();
        drop(out_file);

        match in_node_kind {
            NodeKind::File | NodeKind::BlockDevice => {}
            NodeKind::Directory => return Err(SysErr::IsDir),
            _ => return Err(SysErr::Inval),
        }
        match out_node_kind {
            NodeKind::Directory => return Err(SysErr::IsDir),
            NodeKind::Fifo => return Err(SysErr::SPipe),
            NodeKind::File | NodeKind::BlockDevice | NodeKind::CharDevice => {}
            _ => return Err(SysErr::Inval),
        }

        let mut input_offset = self.read_optional_file_offset(off_in_ptr)?;
        let mut output_offset = self.read_optional_file_offset(off_out_ptr)?;
        let mut total = 0usize;
        let mut remaining = core::cmp::min(len, MAX_RW_COUNT);
        let mut chunk = vec![0u8; core::cmp::min(remaining, COPY_FILE_RANGE_CHUNK_SIZE)];

        while remaining != 0 {
            let wanted = core::cmp::min(remaining, chunk.len());
            let read = if let Some(position) = input_offset {
                match in_node.read(position, &mut chunk[..wanted]) {
                    Ok(read) => read,
                    Err(FsError::WouldBlock) if total != 0 || in_nonblock => break,
                    Err(FsError::WouldBlock) => {
                        return Ok(CopyFileRangeStep::WouldBlock(CopyFileRangeBlock::Read));
                    }
                    Err(_) if total != 0 => break,
                    Err(error) => return Err(SysErr::from(error)),
                }
            } else {
                match in_file_ref.lock().read(&mut chunk[..wanted]) {
                    Ok(read) => read,
                    Err(FsError::WouldBlock) if total != 0 || in_nonblock => break,
                    Err(FsError::WouldBlock) => {
                        return Ok(CopyFileRangeStep::WouldBlock(CopyFileRangeBlock::Read));
                    }
                    Err(_) if total != 0 => break,
                    Err(error) => return Err(SysErr::from(error)),
                }
            };
            if read == 0 {
                break;
            }

            let written = if let Some(position) = output_offset {
                match out_file_ref.lock().node().write(position, &chunk[..read]) {
                    Ok(written) => written,
                    Err(FsError::WouldBlock) if total != 0 || out_nonblock => break,
                    Err(FsError::WouldBlock) => {
                        return Ok(CopyFileRangeStep::WouldBlock(CopyFileRangeBlock::Write));
                    }
                    Err(_) if total != 0 => break,
                    Err(error) => return Err(SysErr::from(error)),
                }
            } else {
                match out_file_ref.lock().write(&chunk[..read]) {
                    Ok(written) => written,
                    Err(FsError::WouldBlock) if total != 0 || out_nonblock => break,
                    Err(FsError::WouldBlock) => {
                        return Ok(CopyFileRangeStep::WouldBlock(CopyFileRangeBlock::Write));
                    }
                    Err(_) if total != 0 => break,
                    Err(error) => return Err(SysErr::from(error)),
                }
            };

            total = total.saturating_add(written);
            remaining = remaining.saturating_sub(written);
            if let Some(position) = input_offset.as_mut() {
                *position = position.saturating_add(read);
            }
            if let Some(position) = output_offset.as_mut() {
                *position = position.saturating_add(written);
            }
            if written < read {
                break;
            }
        }

        self.write_optional_file_offset(off_in_ptr, input_offset)?;
        self.write_optional_file_offset(off_out_ptr, output_offset)?;
        Ok(CopyFileRangeStep::Complete(total as u64))
    }

    fn read_optional_file_offset(&self, address: u64) -> SysResult<Option<usize>> {
        if address == 0 {
            return Ok(None);
        }
        let bytes = self.read_user_exact_buffer(address, 8)?;
        let offset = i64::from_ne_bytes(bytes[..8].try_into().map_err(|_| SysErr::Fault)?);
        if offset < 0 {
            return Err(SysErr::Inval);
        }
        Ok(Some(offset as usize))
    }

    fn write_optional_file_offset(&mut self, address: u64, offset: Option<usize>) -> SysResult<()> {
        if let Some(offset) = offset {
            self.write_user_buffer(address, &(offset as i64).to_ne_bytes())?;
        }
        Ok(())
    }
}
