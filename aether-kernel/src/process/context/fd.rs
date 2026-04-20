#![allow(dead_code)]

use alloc::sync::Arc;

use aether_frame::time;
use aether_vfs::FileAdvice;

use super::*;
use crate::fs::FileDescriptor;
use crate::net::{
    AcceptedSocket, KernelSocket, SCM_CREDENTIALS, SCM_RIGHTS, SocketCredentials, SocketFile,
    SocketMessage, SocketReceive,
};
use crate::rootfs::{FsLocation, ProcessFsContext};
use crate::signal::SigSet;
use crate::syscall::abi::LinuxTimespec;

const IOV_MAX: usize = 1024;
const MAX_RW_COUNT: usize = 0x7fff_f000;
const POLLFD_SIZE: usize = 8;

const POLLIN: i16 = 0x001;
const POLLPRI: i16 = 0x002;
const POLLOUT: i16 = 0x004;
const POLLERR: i16 = 0x008;
const POLLHUP: i16 = 0x010;
const POLLNVAL: i16 = 0x020;
const POLLRDHUP: i16 = 0x2000;
const POLLRDNORM: i16 = 0x040;
const POLLRDBAND: i16 = 0x080;
const POLLWRNORM: i16 = 0x100;
const POLLWRBAND: i16 = 0x200;
const MSG_PEEK: u64 = 0x0002;
const MSG_CTRUNC: u32 = 0x0008;
const MSG_TRUNC: u32 = 0x0020;
const MSG_DONTWAIT: u64 = 0x0040;
const MSG_CMSG_CLOEXEC: u64 = 0x40000000;
const SOCK_NONBLOCK: u64 = 0o0004000;
const SOCK_CLOEXEC: u64 = 0o2000000;
const ACCEPT4_FLAGS_MASK: u64 = SOCK_NONBLOCK | SOCK_CLOEXEC;
const SCM_MAX_FD: usize = 253;
const FD_SET_LIMIT: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, Default)]
struct LinuxPollFd {
    fd: i32,
    events: i16,
    revents: i16,
}

impl LinuxPollFd {
    fn read_from(
        ctx: &ProcessSyscallContext<'_, impl ProcessServices>,
        address: u64,
    ) -> SysResult<Self> {
        let bytes = ctx.syscall_read_user_exact_buffer(address, POLLFD_SIZE)?;
        Ok(Self {
            fd: i32::from_ne_bytes(bytes[0..4].try_into().map_err(|_| SysErr::Fault)?),
            events: i16::from_ne_bytes(bytes[4..6].try_into().map_err(|_| SysErr::Fault)?),
            revents: i16::from_ne_bytes(bytes[6..8].try_into().map_err(|_| SysErr::Fault)?),
        })
    }

    fn to_bytes(self) -> [u8; POLLFD_SIZE] {
        let mut bytes = [0u8; POLLFD_SIZE];
        bytes[0..4].copy_from_slice(&self.fd.to_ne_bytes());
        bytes[4..6].copy_from_slice(&self.events.to_ne_bytes());
        bytes[6..8].copy_from_slice(&self.revents.to_ne_bytes());
        bytes
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct PollWaitOptions {
    deadline_nanos: Option<u64>,
    timeout_nanos: Option<u64>,
    timeout_address: Option<u64>,
    restore_sigmask: Option<SigSet>,
}

#[derive(Debug, Clone, Copy, Default)]
struct LinuxPselectSigmaskArg {
    sigmask: u64,
    sigsetsize: usize,
}

impl LinuxPselectSigmaskArg {
    const SIZE: usize = 16;

    fn read_from(
        ctx: &ProcessSyscallContext<'_, impl ProcessServices>,
        address: u64,
    ) -> SysResult<Self> {
        let bytes = ctx.syscall_read_user_exact_buffer(address, Self::SIZE)?;
        Ok(Self {
            sigmask: u64::from_ne_bytes(bytes[0..8].try_into().map_err(|_| SysErr::Fault)?),
            sigsetsize: u64::from_ne_bytes(bytes[8..16].try_into().map_err(|_| SysErr::Fault)?)
                .min(usize::MAX as u64) as usize,
        })
    }
}

#[derive(Debug, Clone, Copy)]
struct LinuxMsghdr {
    name: u64,
    name_len: u32,
    iov: u64,
    iov_len: usize,
    control: u64,
    control_len: usize,
    flags: u32,
}

impl LinuxMsghdr {
    const SIZE: usize = 56;

    fn read_from(
        ctx: &ProcessSyscallContext<'_, impl ProcessServices>,
        address: u64,
    ) -> SysResult<Self> {
        let bytes = ctx.syscall_read_user_exact_buffer(address, Self::SIZE)?;
        Ok(Self {
            name: u64::from_ne_bytes(bytes[0..8].try_into().map_err(|_| SysErr::Fault)?),
            name_len: u32::from_ne_bytes(bytes[8..12].try_into().map_err(|_| SysErr::Fault)?),
            iov: u64::from_ne_bytes(bytes[16..24].try_into().map_err(|_| SysErr::Fault)?),
            iov_len: u64::from_ne_bytes(bytes[24..32].try_into().map_err(|_| SysErr::Fault)?)
                .min(usize::MAX as u64) as usize,
            control: u64::from_ne_bytes(bytes[32..40].try_into().map_err(|_| SysErr::Fault)?),
            control_len: u64::from_ne_bytes(bytes[40..48].try_into().map_err(|_| SysErr::Fault)?)
                .min(usize::MAX as u64) as usize,
            flags: u32::from_ne_bytes(bytes[48..52].try_into().map_err(|_| SysErr::Fault)?),
        })
    }

    fn write_back(
        self,
        ctx: &mut ProcessSyscallContext<'_, impl ProcessServices>,
        address: u64,
    ) -> SysResult<()> {
        let mut bytes = [0u8; Self::SIZE];
        bytes[0..8].copy_from_slice(&self.name.to_ne_bytes());
        bytes[8..12].copy_from_slice(&self.name_len.to_ne_bytes());
        bytes[16..24].copy_from_slice(&self.iov.to_ne_bytes());
        bytes[24..32].copy_from_slice(&(self.iov_len as u64).to_ne_bytes());
        bytes[32..40].copy_from_slice(&self.control.to_ne_bytes());
        bytes[40..48].copy_from_slice(&(self.control_len as u64).to_ne_bytes());
        bytes[48..52].copy_from_slice(&self.flags.to_ne_bytes());
        ctx.write_user_buffer(address, &bytes)
    }
}

const fn cmsg_header_len() -> usize {
    core::mem::size_of::<usize>() + core::mem::size_of::<i32>() * 2
}

const fn cmsg_align(len: usize) -> usize {
    let align = core::mem::size_of::<usize>();
    (len + align - 1) & !(align - 1)
}

const fn cmsg_len(data_len: usize) -> usize {
    cmsg_header_len() + data_len
}

const fn cmsg_space(data_len: usize) -> usize {
    cmsg_align(cmsg_len(data_len))
}

fn serialize_cmsg(level: i32, kind: i32, payload: &[u8]) -> Vec<u8> {
    let used = cmsg_len(payload.len());
    let total = cmsg_space(payload.len());
    let mut bytes = vec![0u8; total];
    let len_bytes = used.to_ne_bytes();
    bytes[..core::mem::size_of::<usize>()].copy_from_slice(&len_bytes);
    let level_offset = core::mem::size_of::<usize>();
    bytes[level_offset..level_offset + 4].copy_from_slice(&level.to_ne_bytes());
    bytes[level_offset + 4..level_offset + 8].copy_from_slice(&kind.to_ne_bytes());
    bytes[cmsg_header_len()..used].copy_from_slice(payload);
    bytes
}

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn should_not_block_socket_io(&self, fd: u32, flags: u64) -> bool {
        (flags & MSG_DONTWAIT) != 0
            || self
                .process
                .files
                .get(fd)
                .map(|descriptor| descriptor.file.lock().flags().nonblock())
                .unwrap_or(false)
    }

    pub(super) fn syscall_poll(&mut self, fds: u64, nfds: usize, _timeout: i32) -> SysResult<u64> {
        let mut poll_fds = self.read_poll_fds(fds, nfds)?;
        let ready = self.evaluate_poll_fds(&mut poll_fds)?;
        self.write_poll_fds(fds, &poll_fds)?;
        if ready == 0 {
            return Err(SysErr::Again);
        }
        Ok(ready as u64)
    }

    pub(super) fn syscall_poll_blocking(
        &mut self,
        fds: u64,
        nfds: usize,
        timeout: i32,
    ) -> SyscallDisposition {
        let poll_fds = match self.read_poll_fds(fds, nfds) {
            Ok(poll_fds) => poll_fds,
            Err(error) => return SyscallDisposition::err(error),
        };

        self.poll_wait_loop(
            fds,
            poll_fds,
            PollWaitOptions {
                deadline_nanos: (timeout > 0).then(|| {
                    time::MonotonicInstant::now()
                        .saturating_add_nanos(timeout as u64 * 1_000_000)
                        .as_nanos()
                }),
                timeout_nanos: (timeout >= 0).then(|| (timeout as u64).saturating_mul(1_000_000)),
                ..PollWaitOptions::default()
            },
        )
    }

    pub(super) fn syscall_ppoll(
        &mut self,
        fds: u64,
        nfds: usize,
        timeout: u64,
        sigmask: u64,
        sigsetsize: usize,
    ) -> SysResult<u64> {
        let mut options = self.parse_ppoll_timeout(timeout)?;
        options.restore_sigmask = self.parse_ppoll_sigmask(sigmask, sigsetsize)?;

        let result = (|| {
            let mut poll_fds = self.read_poll_fds(fds, nfds)?;
            let ready = self.evaluate_poll_fds(&mut poll_fds)?;
            self.write_poll_fds(fds, &poll_fds)?;
            if ready == 0 {
                return Err(SysErr::Again);
            }
            Ok(ready as u64)
        })();

        self.restore_poll_wait_state(options.restore_sigmask, options.timeout_address, options);
        result
    }

    pub(super) fn syscall_ppoll_blocking(
        &mut self,
        fds: u64,
        nfds: usize,
        timeout: u64,
        sigmask: u64,
        sigsetsize: usize,
    ) -> SyscallDisposition {
        let mut options = match self.parse_ppoll_timeout(timeout) {
            Ok(options) => options,
            Err(error) => return SyscallDisposition::err(error),
        };
        options.restore_sigmask = match self.parse_ppoll_sigmask(sigmask, sigsetsize) {
            Ok(mask) => mask,
            Err(error) => return SyscallDisposition::err(error),
        };

        let poll_fds = match self.read_poll_fds(fds, nfds) {
            Ok(poll_fds) => poll_fds,
            Err(error) => {
                self.restore_poll_wait_state(
                    options.restore_sigmask,
                    options.timeout_address,
                    options,
                );
                return SyscallDisposition::err(error);
            }
        };

        self.poll_wait_loop(fds, poll_fds, options)
    }

    pub(super) fn syscall_pselect6(
        &mut self,
        nfds: i32,
        readfds: u64,
        writefds: u64,
        exceptfds: u64,
        timeout: u64,
        sigmask: u64,
    ) -> SysResult<u64> {
        let nfds = usize::try_from(nfds).map_err(|_| SysErr::Inval)?;
        let mut options = self.parse_ppoll_timeout(timeout)?;
        options.restore_sigmask = self.parse_pselect6_sigmask(sigmask)?;

        let result = (|| {
            let (mut read_set, mut write_set, mut except_set) =
                self.read_select_fd_sets(nfds, readfds, writefds, exceptfds)?;
            let ready = self.evaluate_select_fd_sets(
                nfds,
                read_set.as_mut_slice(),
                write_set.as_mut_slice(),
                except_set.as_mut_slice(),
            )?;
            self.write_select_fd_sets(
                readfds,
                writefds,
                exceptfds,
                &read_set,
                &write_set,
                &except_set,
            )?;
            if ready == 0 {
                return Err(SysErr::Again);
            }
            Ok(ready as u64)
        })();

        self.restore_poll_wait_state(options.restore_sigmask, options.timeout_address, options);
        result
    }

    pub(super) fn syscall_pselect6_blocking(
        &mut self,
        nfds: i32,
        readfds: u64,
        writefds: u64,
        exceptfds: u64,
        timeout: u64,
        sigmask: u64,
    ) -> SyscallDisposition {
        let nfds = match usize::try_from(nfds) {
            Ok(nfds) => nfds,
            Err(_) => return SyscallDisposition::err(SysErr::Inval),
        };
        let mut options = match self.parse_ppoll_timeout(timeout) {
            Ok(options) => options,
            Err(error) => return SyscallDisposition::err(error),
        };
        options.restore_sigmask = match self.parse_pselect6_sigmask(sigmask) {
            Ok(mask) => mask,
            Err(error) => return SyscallDisposition::err(error),
        };

        let (read_set, write_set, except_set) =
            match self.read_select_fd_sets(nfds, readfds, writefds, exceptfds) {
                Ok(sets) => sets,
                Err(error) => {
                    self.restore_poll_wait_state(
                        options.restore_sigmask,
                        options.timeout_address,
                        options,
                    );
                    return SyscallDisposition::err(error);
                }
            };

        self.pselect_wait_loop(
            nfds, readfds, writefds, exceptfds, read_set, write_set, except_set, options,
        )
    }

    fn parse_pselect6_sigmask(&mut self, sigmask: u64) -> SysResult<Option<SigSet>> {
        if sigmask == 0 {
            return Ok(None);
        }

        let argument = LinuxPselectSigmaskArg::read_from(self, sigmask)?;
        if argument.sigmask == 0 {
            return Ok(None);
        }
        self.parse_ppoll_sigmask(argument.sigmask, argument.sigsetsize)
    }

    fn pselect_wait_loop(
        &mut self,
        nfds: usize,
        readfds_address: u64,
        writefds_address: u64,
        exceptfds_address: u64,
        read_set: Vec<u8>,
        write_set: Vec<u8>,
        except_set: Vec<u8>,
        options: PollWaitOptions,
    ) -> SyscallDisposition {
        let registrations =
            self.collect_select_registrations(nfds, &read_set, &write_set, &except_set);

        loop {
            let mut current_read_set = read_set.clone();
            let mut current_write_set = write_set.clone();
            let mut current_except_set = except_set.clone();
            let ready = match self.evaluate_select_fd_sets(
                nfds,
                current_read_set.as_mut_slice(),
                current_write_set.as_mut_slice(),
                current_except_set.as_mut_slice(),
            ) {
                Ok(ready) => ready,
                Err(error) => {
                    self.restore_poll_wait_state(
                        options.restore_sigmask,
                        options.timeout_address,
                        options,
                    );
                    return SyscallDisposition::err(error);
                }
            };
            if let Err(error) = self.write_select_fd_sets(
                readfds_address,
                writefds_address,
                exceptfds_address,
                &current_read_set,
                &current_write_set,
                &current_except_set,
            ) {
                self.restore_poll_wait_state(
                    options.restore_sigmask,
                    options.timeout_address,
                    options,
                );
                return SyscallDisposition::err(error);
            }
            if ready != 0 {
                self.restore_poll_wait_state(
                    options.restore_sigmask,
                    options.timeout_address,
                    options,
                );
                return SyscallDisposition::ok(ready as u64);
            }
            if options.timeout_nanos == Some(0) {
                self.restore_poll_wait_state(
                    options.restore_sigmask,
                    options.timeout_address,
                    options,
                );
                return SyscallDisposition::ok(0);
            }
            if self
                .process
                .signals
                .has_deliverable(crate::arch::supports_user_handlers())
            {
                self.restore_poll_wait_state(
                    options.restore_sigmask,
                    options.timeout_address,
                    options,
                );
                return SyscallDisposition::err(SysErr::Intr);
            }

            match self.wait_poll(options.deadline_nanos, &registrations) {
                Ok(BlockResult::Poll { timed_out: false }) => {}
                Ok(BlockResult::Poll { timed_out: true }) => {
                    let ready = match self.evaluate_select_fd_sets(
                        nfds,
                        current_read_set.as_mut_slice(),
                        current_write_set.as_mut_slice(),
                        current_except_set.as_mut_slice(),
                    ) {
                        Ok(ready) => ready,
                        Err(error) => {
                            self.restore_poll_wait_state(
                                options.restore_sigmask,
                                options.timeout_address,
                                options,
                            );
                            return SyscallDisposition::err(error);
                        }
                    };
                    if let Err(error) = self.write_select_fd_sets(
                        readfds_address,
                        writefds_address,
                        exceptfds_address,
                        &current_read_set,
                        &current_write_set,
                        &current_except_set,
                    ) {
                        self.restore_poll_wait_state(
                            options.restore_sigmask,
                            options.timeout_address,
                            options,
                        );
                        return SyscallDisposition::err(error);
                    }
                    self.restore_poll_wait_state(
                        options.restore_sigmask,
                        options.timeout_address,
                        options,
                    );
                    return SyscallDisposition::ok(ready as u64);
                }
                Ok(BlockResult::SignalInterrupted) => {
                    self.restore_poll_wait_state(
                        options.restore_sigmask,
                        options.timeout_address,
                        options,
                    );
                    return SyscallDisposition::err(SysErr::Intr);
                }
                Ok(_) => {
                    self.restore_poll_wait_state(
                        options.restore_sigmask,
                        options.timeout_address,
                        options,
                    );
                    return SyscallDisposition::err(SysErr::Intr);
                }
                Err(disposition) => return disposition,
            }
        }
    }

    fn parse_ppoll_timeout(&self, timeout: u64) -> SysResult<PollWaitOptions> {
        if timeout == 0 {
            return Ok(PollWaitOptions::default());
        }

        let request = LinuxTimespec::read_from(self, timeout)?.validate()?;
        let timeout_nanos = request.total_nanos()?;
        Ok(PollWaitOptions {
            deadline_nanos: Some(
                time::MonotonicInstant::now()
                    .saturating_add_nanos(timeout_nanos)
                    .as_nanos(),
            ),
            timeout_nanos: Some(timeout_nanos),
            timeout_address: Some(timeout),
            restore_sigmask: None,
        })
    }

    fn parse_ppoll_sigmask(
        &mut self,
        sigmask: u64,
        sigsetsize: usize,
    ) -> SysResult<Option<SigSet>> {
        if sigmask == 0 {
            return Ok(None);
        }
        if sigsetsize != core::mem::size_of::<SigSet>() {
            return Err(SysErr::Inval);
        }

        let previous = self.process.signals.blocked();
        let raw = self.read_user_buffer(sigmask, sigsetsize)?;
        self.process
            .signals
            .set_blocked_mask(crate::process::decode_sigset(&raw));
        Ok(Some(previous))
    }

    fn poll_wait_loop(
        &mut self,
        user_fds: u64,
        mut poll_fds: Vec<LinuxPollFd>,
        options: PollWaitOptions,
    ) -> SyscallDisposition {
        let registrations = self.collect_poll_registrations(&poll_fds);

        loop {
            let ready = match self.evaluate_poll_fds(&mut poll_fds) {
                Ok(ready) => ready,
                Err(error) => {
                    self.restore_poll_wait_state(
                        options.restore_sigmask,
                        options.timeout_address,
                        options,
                    );
                    return SyscallDisposition::err(error);
                }
            };
            if let Err(error) = self.write_poll_fds(user_fds, &poll_fds) {
                self.restore_poll_wait_state(
                    options.restore_sigmask,
                    options.timeout_address,
                    options,
                );
                return SyscallDisposition::err(error);
            }
            if ready != 0 {
                self.restore_poll_wait_state(
                    options.restore_sigmask,
                    options.timeout_address,
                    options,
                );
                return SyscallDisposition::ok(ready as u64);
            }
            if options.timeout_nanos == Some(0) {
                self.restore_poll_wait_state(
                    options.restore_sigmask,
                    options.timeout_address,
                    options,
                );
                return SyscallDisposition::ok(0);
            }
            if self
                .process
                .signals
                .has_deliverable(crate::arch::supports_user_handlers())
            {
                self.restore_poll_wait_state(
                    options.restore_sigmask,
                    options.timeout_address,
                    options,
                );
                return SyscallDisposition::err(SysErr::Intr);
            }

            match self.wait_poll(options.deadline_nanos, &registrations) {
                Ok(BlockResult::Poll { timed_out: false }) => {}
                Ok(BlockResult::Poll { timed_out: true }) => {
                    let ready = match self.evaluate_poll_fds(&mut poll_fds) {
                        Ok(ready) => ready,
                        Err(error) => {
                            self.restore_poll_wait_state(
                                options.restore_sigmask,
                                options.timeout_address,
                                options,
                            );
                            return SyscallDisposition::err(error);
                        }
                    };
                    if let Err(error) = self.write_poll_fds(user_fds, &poll_fds) {
                        self.restore_poll_wait_state(
                            options.restore_sigmask,
                            options.timeout_address,
                            options,
                        );
                        return SyscallDisposition::err(error);
                    }
                    self.restore_poll_wait_state(
                        options.restore_sigmask,
                        options.timeout_address,
                        options,
                    );
                    return SyscallDisposition::ok(ready as u64);
                }
                Ok(BlockResult::SignalInterrupted) => {
                    self.restore_poll_wait_state(
                        options.restore_sigmask,
                        options.timeout_address,
                        options,
                    );
                    return SyscallDisposition::err(SysErr::Intr);
                }
                Ok(_) => {
                    self.restore_poll_wait_state(
                        options.restore_sigmask,
                        options.timeout_address,
                        options,
                    );
                    return SyscallDisposition::err(SysErr::Intr);
                }
                Err(disposition) => return disposition,
            }
        }
    }

    pub(super) fn syscall_sendto(
        &mut self,
        fd: u64,
        buffer: u64,
        len: usize,
        flags: u64,
        address: u64,
        address_len: usize,
    ) -> SysResult<u64> {
        let (_file_ref, socket) = self.socket_from_fd(fd)?;
        let bytes = self.read_user_buffer(buffer, len)?;
        let address = self.read_optional_socket_address(address, address_len)?;
        let peer = address
            .as_deref()
            .map(|address| self.resolve_socket_address_target(address))
            .transpose()?
            .flatten();
        socket
            .send_to_socket(bytes.as_slice(), address.as_deref(), flags, peer)
            .map(|written| written as u64)
    }

    pub(super) fn syscall_sendto_blocking(
        &mut self,
        fd: u64,
        buffer: u64,
        len: usize,
        flags: u64,
        address: u64,
        address_len: usize,
    ) -> SyscallDisposition {
        if self.should_not_block_socket_io(fd as u32, flags) {
            return SyscallDisposition::Return(self.syscall_sendto(
                fd,
                buffer,
                len,
                flags,
                address,
                address_len,
            ));
        }
        self.file_blocking_syscall(fd as u32, PollEvents::WRITE, |ctx| {
            ctx.syscall_sendto(fd, buffer, len, flags, address, address_len)
        })
    }

    pub(super) fn syscall_recvfrom(
        &mut self,
        fd: u64,
        buffer: u64,
        len: usize,
        flags: u64,
        address: u64,
        address_len: u64,
    ) -> SysResult<u64> {
        let (_file_ref, socket) = self.socket_from_fd(fd)?;
        if address != 0 && address_len == 0 {
            return Err(SysErr::Fault);
        }
        let mut bytes = vec![0u8; len];
        let received = socket.recv_from(bytes.as_mut_slice(), flags)?;
        self.write_user_buffer(buffer, &bytes[..received.bytes_read])?;
        self.write_socket_receive_address(address, address_len, &received)?;
        Ok(received.bytes_read as u64)
    }

    pub(super) fn syscall_recvfrom_blocking(
        &mut self,
        fd: u64,
        buffer: u64,
        len: usize,
        flags: u64,
        address: u64,
        address_len: u64,
    ) -> SyscallDisposition {
        if self.should_not_block_socket_io(fd as u32, flags) {
            return SyscallDisposition::Return(self.syscall_recvfrom(
                fd,
                buffer,
                len,
                flags,
                address,
                address_len,
            ));
        }
        self.file_blocking_syscall(fd as u32, PollEvents::READ, |ctx| {
            ctx.syscall_recvfrom(fd, buffer, len, flags, address, address_len)
        })
    }

    pub(super) fn syscall_sendmsg(&mut self, fd: u64, message: u64, flags: u64) -> SysResult<u64> {
        let (_file_ref, socket) = self.socket_from_fd(fd)?;
        let message = self.read_socket_message(message)?;
        let peer = message
            .name
            .as_deref()
            .map(|address| self.resolve_socket_address_target(address))
            .transpose()?
            .flatten();
        socket
            .send_msg_to_socket(&message, flags, peer)
            .map(|written| written as u64)
    }

    pub(super) fn syscall_recvmsg(&mut self, fd: u64, message: u64, flags: u64) -> SysResult<u64> {
        let (_file_ref, socket) = self.socket_from_fd(fd)?;
        let mut header = LinuxMsghdr::read_from(self, message)?;
        if header.iov_len > IOV_MAX {
            return Err(SysErr::Inval);
        }

        let segments = super::super::util::read_iovec_array(
            &self.process.task.address_space,
            header.iov,
            header.iov_len,
        )?;
        let total_len = segments.iter().try_fold(0usize, |total, segment| {
            total
                .checked_add(segment.len)
                .filter(|next| *next <= MAX_RW_COUNT)
                .ok_or(SysErr::Inval)
        })?;

        let mut bytes = vec![0u8; total_len];
        let received = socket.recv_msg(bytes.as_mut_slice(), flags)?;
        self.write_iovec_bytes(&segments, &bytes[..received.bytes_read])?;
        self.write_socket_receive_name(header.name, header.name_len as u64, &received)?;
        let (control_len, control_flags) = self.write_socket_receive_control(
            header.control,
            header.control_len,
            &received,
            flags,
        )?;

        header.name_len = received
            .address
            .as_ref()
            .filter(|_| header.name != 0)
            .map(|name| name.len() as u32)
            .unwrap_or(0);
        header.control_len = control_len;
        header.flags = received.msg_flags | control_flags;
        header.write_back(self, message)?;
        Ok(received.bytes_read as u64)
    }

    pub(super) fn syscall_sendmsg_blocking(
        &mut self,
        fd: u64,
        message: u64,
        flags: u64,
    ) -> SyscallDisposition {
        if self.should_not_block_socket_io(fd as u32, flags) {
            return SyscallDisposition::Return(self.syscall_sendmsg(fd, message, flags));
        }
        self.file_blocking_syscall(fd as u32, PollEvents::WRITE, |ctx| {
            ctx.syscall_sendmsg(fd, message, flags)
        })
    }

    pub(super) fn syscall_recvmsg_blocking(
        &mut self,
        fd: u64,
        message: u64,
        flags: u64,
    ) -> SyscallDisposition {
        if self.should_not_block_socket_io(fd as u32, flags) {
            return SyscallDisposition::Return(self.syscall_recvmsg(fd, message, flags));
        }
        self.file_blocking_syscall(fd as u32, PollEvents::READ, |ctx| {
            ctx.syscall_recvmsg(fd, message, flags)
        })
    }

    fn create_epoll_fd(&mut self, cloexec: bool) -> SysResult<u64> {
        let epoll = aether_vfs::create_epoll_instance();
        let node: aether_vfs::NodeRef = aether_vfs::FileNode::new("epoll", epoll);
        let filesystem = super::super::util::anonymous_filesystem_identity();
        Ok(self.process.files.insert_node(
            node,
            aether_vfs::OpenFlags::from_bits(aether_vfs::OpenFlags::READ),
            filesystem,
            None,
            cloexec,
        ) as u64)
    }

    pub(super) fn syscall_epoll_create(&mut self, size: u64) -> SysResult<u64> {
        if size == 0 {
            return Err(SysErr::Inval);
        }
        self.create_epoll_fd(false)
    }

    pub(super) fn syscall_epoll_create1(&mut self, flags: u64) -> SysResult<u64> {
        const EPOLL_CLOEXEC: u64 = 0o2000000;

        if (flags & !EPOLL_CLOEXEC) != 0 {
            return Err(SysErr::Inval);
        }
        self.create_epoll_fd((flags & EPOLL_CLOEXEC) != 0)
    }

    pub(super) fn syscall_epoll_ctl(
        &mut self,
        epfd: u64,
        op: i32,
        fd: u64,
        event: u64,
    ) -> SysResult<u64> {
        let epoll_op = aether_vfs::EpollCtlOp::from_raw(op).ok_or(SysErr::Inval)?;

        let epoll_descriptor = self.process.files.get(epfd as u32).ok_or(SysErr::BadFd)?;
        let epoll_guard = epoll_descriptor.file.lock();
        let epoll_file = epoll_guard
            .file_ops()
            .and_then(|ops| ops.as_any().downcast_ref::<aether_vfs::EpollInstance>())
            .ok_or(SysErr::Inval)?;

        let target_descriptor = self.process.files.get(fd as u32).ok_or(SysErr::BadFd)?;
        let target_node = target_descriptor.file.lock().node();

        let epoll_event = if event != 0 {
            let event_bytes = self.syscall_read_user_exact_buffer(event, 12)?;
            aether_vfs::EpollEvent::from_bytes(&event_bytes.try_into().unwrap_or([0; 12]))
        } else {
            aether_vfs::EpollEvent::default()
        };

        epoll_file.ctl(epoll_op, fd, target_node, epoll_event)?;
        Ok(0)
    }

    pub(super) fn syscall_epoll_wait(
        &mut self,
        epfd: u64,
        events: u64,
        maxevents: usize,
        timeout: i32,
    ) -> SysResult<u64> {
        self.syscall_epoll_pwait(epfd, events, maxevents, timeout, 0)
    }

    pub(super) fn syscall_epoll_wait_blocking(
        &mut self,
        epfd: u64,
        events: u64,
        maxevents: usize,
        timeout: i32,
    ) -> SyscallDisposition {
        self.syscall_epoll_pwait_blocking(epfd, events, maxevents, timeout, 0)
    }

    pub(super) fn syscall_epoll_pwait(
        &mut self,
        epfd: u64,
        events: u64,
        maxevents: usize,
        timeout: i32,
        sigmask: u64,
    ) -> SysResult<u64> {
        if maxevents == 0 {
            return Err(SysErr::Inval);
        }

        let options = self.parse_epoll_timeout(timeout)?;
        let restore_sigmask = self.parse_epoll_sigmask(sigmask)?;

        let result = (|| {
            let epoll_descriptor = self.process.files.get(epfd as u32).ok_or(SysErr::BadFd)?;
            let epoll_guard = epoll_descriptor.file.lock();
            let epoll_file = epoll_guard
                .file_ops()
                .and_then(|ops| ops.as_any().downcast_ref::<aether_vfs::EpollInstance>())
                .ok_or(SysErr::Inval)?;

            let ready_events = epoll_file.wait(maxevents)?;

            drop(epoll_guard);

            if ready_events.is_empty() {
                return Err(SysErr::Again);
            }

            for (i, ready_event) in ready_events.iter().enumerate() {
                if i >= maxevents {
                    break;
                }
                let event_bytes = ready_event.to_bytes();
                self.write_user_buffer(events + (i * 12) as u64, &event_bytes)?;
            }

            Ok(ready_events.len().min(maxevents) as u64)
        })();

        self.restore_epoll_wait_state(restore_sigmask, options);
        result
    }

    pub(super) fn syscall_epoll_pwait_blocking(
        &mut self,
        epfd: u64,
        events: u64,
        maxevents: usize,
        timeout: i32,
        sigmask: u64,
    ) -> SyscallDisposition {
        if maxevents == 0 {
            return SyscallDisposition::err(SysErr::Inval);
        }

        let options = match self.parse_epoll_timeout(timeout) {
            Ok(options) => options,
            Err(error) => return SyscallDisposition::err(error),
        };

        let restore_sigmask = match self.parse_epoll_sigmask(sigmask) {
            Ok(mask) => mask,
            Err(error) => {
                self.restore_epoll_wait_state(None, options);
                return SyscallDisposition::err(error);
            }
        };

        self.epoll_wait_loop(epfd as u32, events, maxevents, options, restore_sigmask)
    }

    fn parse_epoll_timeout(&self, timeout: i32) -> SysResult<PollWaitOptions> {
        if timeout == -1 {
            return Ok(PollWaitOptions::default());
        }

        let timeout_nanos = if timeout < 0 {
            return Err(SysErr::Inval);
        } else {
            (timeout as u64).saturating_mul(1_000_000)
        };

        let deadline_nanos = if timeout_nanos == 0 {
            None
        } else {
            Some(
                time::MonotonicInstant::now()
                    .saturating_add_nanos(timeout_nanos)
                    .as_nanos(),
            )
        };

        Ok(PollWaitOptions {
            deadline_nanos,
            timeout_nanos: Some(timeout_nanos),
            timeout_address: None,
            restore_sigmask: None,
        })
    }

    fn parse_epoll_sigmask(&mut self, sigmask: u64) -> SysResult<Option<SigSet>> {
        if sigmask == 0 {
            return Ok(None);
        }

        let previous = self.process.signals.blocked();
        let raw = self.read_user_buffer(sigmask, core::mem::size_of::<SigSet>())?;
        self.process
            .signals
            .set_blocked_mask(crate::process::decode_sigset(&raw));
        Ok(Some(previous))
    }

    fn epoll_wait_loop(
        &mut self,
        epfd: u32,
        events: u64,
        maxevents: usize,
        options: PollWaitOptions,
        restore_sigmask: Option<SigSet>,
    ) -> SyscallDisposition {
        let registrations = [crate::process::PendingPollRegistration {
            fd: epfd,
            events: PollEvents::READ,
        }];

        loop {
            let ready_events = match self.collect_epoll_ready_events(epfd, maxevents) {
                Ok(events) => events,
                Err(error) => {
                    self.restore_epoll_wait_state(restore_sigmask, options);
                    return SyscallDisposition::err(error);
                }
            };
            if !ready_events.is_empty() {
                if let Err(error) = self.write_epoll_ready_events(events, &ready_events, maxevents)
                {
                    self.restore_epoll_wait_state(restore_sigmask, options);
                    return SyscallDisposition::err(error);
                }
                self.restore_epoll_wait_state(restore_sigmask, options);
                return SyscallDisposition::ok(ready_events.len().min(maxevents) as u64);
            }
            if options.timeout_nanos == Some(0) {
                self.restore_epoll_wait_state(restore_sigmask, options);
                return SyscallDisposition::ok(0);
            }
            if self
                .process
                .signals
                .has_deliverable(crate::arch::supports_user_handlers())
            {
                self.restore_epoll_wait_state(restore_sigmask, options);
                return SyscallDisposition::err(SysErr::Intr);
            }

            match self.wait_poll(options.deadline_nanos, &registrations) {
                Ok(BlockResult::Poll { timed_out: false }) => {}
                Ok(BlockResult::Poll { timed_out: true }) => {
                    let ready_events = match self.collect_epoll_ready_events(epfd, maxevents) {
                        Ok(events) => events,
                        Err(error) => {
                            self.restore_epoll_wait_state(restore_sigmask, options);
                            return SyscallDisposition::err(error);
                        }
                    };
                    if let Err(error) =
                        self.write_epoll_ready_events(events, &ready_events, maxevents)
                    {
                        self.restore_epoll_wait_state(restore_sigmask, options);
                        return SyscallDisposition::err(error);
                    }
                    self.restore_epoll_wait_state(restore_sigmask, options);
                    return SyscallDisposition::ok(ready_events.len().min(maxevents) as u64);
                }
                Ok(BlockResult::SignalInterrupted) => {
                    self.restore_epoll_wait_state(restore_sigmask, options);
                    return SyscallDisposition::err(SysErr::Intr);
                }
                Ok(_) => {
                    self.restore_epoll_wait_state(restore_sigmask, options);
                    return SyscallDisposition::err(SysErr::Intr);
                }
                Err(disposition) => return disposition,
            }
        }
    }

    fn collect_epoll_ready_events(
        &self,
        epfd: u32,
        maxevents: usize,
    ) -> SysResult<Vec<aether_vfs::EpollEvent>> {
        let epoll_descriptor = self.process.files.get(epfd).ok_or(SysErr::BadFd)?;
        let epoll_guard = epoll_descriptor.file.lock();
        let epoll_file = epoll_guard
            .file_ops()
            .and_then(|ops| ops.as_any().downcast_ref::<aether_vfs::EpollInstance>())
            .ok_or(SysErr::Inval)?;
        epoll_file.wait(maxevents).map_err(SysErr::from)
    }

    fn write_epoll_ready_events(
        &mut self,
        address: u64,
        ready_events: &[aether_vfs::EpollEvent],
        maxevents: usize,
    ) -> SysResult<()> {
        for (i, ready_event) in ready_events.iter().enumerate() {
            if i >= maxevents {
                break;
            }
            let event_bytes = ready_event.to_bytes();
            self.write_user_buffer(address + (i * 12) as u64, &event_bytes)?;
        }
        Ok(())
    }

    fn restore_epoll_wait_state(
        &mut self,
        restore_sigmask: Option<SigSet>,
        options: PollWaitOptions,
    ) {
        if let Some(previous_mask) = restore_sigmask {
            self.process.signals.set_blocked_mask(previous_mask);
        }
        let _ = options;
    }

    fn read_poll_fds(&self, address: u64, nfds: usize) -> SysResult<Vec<LinuxPollFd>> {
        if nfds > FD_SET_LIMIT {
            return Err(SysErr::Inval);
        }

        let mut poll_fds = Vec::with_capacity(nfds);
        for index in 0..nfds {
            let entry_address = address
                .checked_add((index * POLLFD_SIZE) as u64)
                .ok_or(SysErr::Fault)?;
            poll_fds.push(LinuxPollFd::read_from(self, entry_address)?);
        }
        Ok(poll_fds)
    }

    fn write_poll_fds(&mut self, address: u64, poll_fds: &[LinuxPollFd]) -> SysResult<()> {
        for (index, poll_fd) in poll_fds.iter().enumerate() {
            let entry_address = address
                .checked_add((index * POLLFD_SIZE) as u64)
                .ok_or(SysErr::Fault)?;
            self.write_user_buffer(entry_address, &poll_fd.to_bytes())?;
        }
        Ok(())
    }

    fn evaluate_poll_fds(&self, poll_fds: &mut [LinuxPollFd]) -> SysResult<usize> {
        let mut ready_count = 0usize;

        for poll_fd in poll_fds {
            poll_fd.revents = 0;
            if poll_fd.fd < 0 {
                continue;
            }

            let Some(descriptor) = self.process.files.get(poll_fd.fd as u32) else {
                poll_fd.revents = POLLNVAL;
                ready_count += 1;
                continue;
            };

            let requested = linux_poll_to_events(poll_fd.events);
            let ready = descriptor
                .file
                .lock()
                .poll(requested | PollEvents::ALWAYS)
                .map_err(SysErr::from)?;
            poll_fd.revents = events_to_linux_poll(ready);
            if poll_fd.revents != 0 {
                ready_count += 1;
            }
        }

        Ok(ready_count)
    }

    fn collect_poll_registrations(
        &self,
        poll_fds: &[LinuxPollFd],
    ) -> Vec<crate::process::PendingPollRegistration> {
        let mut registrations = Vec::new();

        for poll_fd in poll_fds.iter().copied() {
            if poll_fd.fd < 0 {
                continue;
            }
            let events = linux_poll_to_events(poll_fd.events) | PollEvents::ALWAYS;
            let fd = poll_fd.fd as u32;
            let Some(_descriptor) = self.process.files.get(fd) else {
                continue;
            };
            registrations.push(crate::process::PendingPollRegistration { fd, events });
        }

        registrations
    }

    fn read_select_fd_sets(
        &self,
        nfds: usize,
        readfds: u64,
        writefds: u64,
        exceptfds: u64,
    ) -> SysResult<(Vec<u8>, Vec<u8>, Vec<u8>)> {
        if nfds > FD_SET_LIMIT {
            return Err(SysErr::Inval);
        }
        let bytes = fd_set_bytes_len(nfds);
        Ok((
            self.read_select_fd_set(readfds, bytes)?,
            self.read_select_fd_set(writefds, bytes)?,
            self.read_select_fd_set(exceptfds, bytes)?,
        ))
    }

    fn read_select_fd_set(&self, address: u64, bytes: usize) -> SysResult<Vec<u8>> {
        if bytes == 0 || address == 0 {
            return Ok(vec![0; bytes]);
        }
        self.syscall_read_user_exact_buffer(address, bytes)
    }

    fn write_select_fd_sets(
        &mut self,
        readfds: u64,
        writefds: u64,
        exceptfds: u64,
        read_set: &[u8],
        write_set: &[u8],
        except_set: &[u8],
    ) -> SysResult<()> {
        self.write_select_fd_set(readfds, read_set)?;
        self.write_select_fd_set(writefds, write_set)?;
        self.write_select_fd_set(exceptfds, except_set)?;
        Ok(())
    }

    fn write_select_fd_set(&mut self, address: u64, bytes: &[u8]) -> SysResult<()> {
        if address == 0 || bytes.is_empty() {
            return Ok(());
        }
        self.write_user_buffer(address, bytes)
    }

    fn evaluate_select_fd_sets(
        &self,
        nfds: usize,
        read_set: &mut [u8],
        write_set: &mut [u8],
        except_set: &mut [u8],
    ) -> SysResult<usize> {
        let mut ready_count = 0usize;
        let exceptional_mask =
            PollEvents::ERROR | PollEvents::HUP | PollEvents::RDHUP | PollEvents::INVALID;

        for fd in 0..nfds {
            let want_read = fd_set_test(read_set, fd);
            let want_write = fd_set_test(write_set, fd);
            let want_except = fd_set_test(except_set, fd);
            if !want_read && !want_write && !want_except {
                continue;
            }

            let descriptor = self.process.files.get(fd as u32).ok_or(SysErr::BadFd)?;
            let mut requested = PollEvents::ALWAYS;
            if want_read {
                requested = requested | PollEvents::READ;
            }
            if want_write {
                requested = requested | PollEvents::WRITE;
            }

            let ready = descriptor
                .file
                .lock()
                .poll(requested)
                .map_err(SysErr::from)?;
            let read_ready = want_read
                && (ready.contains(PollEvents::READ) || ready.intersects(exceptional_mask));
            let write_ready = want_write
                && (ready.contains(PollEvents::WRITE) || ready.intersects(exceptional_mask));
            // TODO: Linux exceptfds is driven by POLLPRI-style out-of-band readiness.
            // PollEvents does not expose that signal yet, so we conservatively clear exceptfds.
            let except_ready = false && want_except;

            fd_set_assign(read_set, fd, read_ready);
            fd_set_assign(write_set, fd, write_ready);
            fd_set_assign(except_set, fd, except_ready);

            if read_ready || write_ready || except_ready {
                ready_count += 1;
            }
        }

        Ok(ready_count)
    }

    fn collect_select_registrations(
        &self,
        nfds: usize,
        read_set: &[u8],
        write_set: &[u8],
        except_set: &[u8],
    ) -> Vec<crate::process::PendingPollRegistration> {
        let mut registrations = Vec::new();

        for fd in 0..nfds {
            let want_read = fd_set_test(read_set, fd);
            let want_write = fd_set_test(write_set, fd);
            let want_except = fd_set_test(except_set, fd);
            if !want_read && !want_write && !want_except {
                continue;
            }

            let mut events = PollEvents::ALWAYS;
            if want_read {
                events = events | PollEvents::READ;
            }
            if want_write {
                events = events | PollEvents::WRITE;
            }
            let Some(_descriptor) = self.process.files.get(fd as u32) else {
                continue;
            };
            registrations.push(crate::process::PendingPollRegistration {
                fd: fd as u32,
                events,
            });
        }

        registrations
    }

    fn restore_poll_wait_state(
        &mut self,
        restore_sigmask: Option<SigSet>,
        timeout_address: Option<u64>,
        options: PollWaitOptions,
    ) {
        if let Some(previous_mask) = restore_sigmask {
            self.process.signals.restore_mask(previous_mask);
        }
        if let Some(address) = timeout_address {
            let remaining_nanos = options
                .deadline_nanos
                .map(|deadline| {
                    time::MonotonicInstant::from_nanos(deadline)
                        .saturating_nanos_since(time::MonotonicInstant::now())
                })
                .or(options.timeout_nanos)
                .unwrap_or(0);
            let _ = self.write_user_timespec(
                address,
                (remaining_nanos / 1_000_000_000) as i64,
                (remaining_nanos % 1_000_000_000) as i64,
            );
        }
    }

    pub(crate) fn socket_from_fd(
        &self,
        fd: u64,
    ) -> SysResult<(aether_vfs::SharedOpenFile, Arc<dyn KernelSocket>)> {
        let descriptor = self.process.files.get(fd as u32).ok_or(SysErr::BadFd)?;
        let file_ref = descriptor.file.clone();
        let node = file_ref.lock().node();
        let socket = self.socket_from_node(&node)?;
        Ok((file_ref, socket))
    }

    pub(crate) fn socket_from_node(
        &self,
        node: &aether_vfs::NodeRef,
    ) -> SysResult<Arc<dyn KernelSocket>> {
        let socket = node
            .file()
            .and_then(|file| file.as_any().downcast_ref::<SocketFile>())
            .map(SocketFile::socket)
            .ok_or(SysErr::NotSock)?;
        Ok(socket)
    }

    pub(crate) fn resolve_socket_address_target(
        &mut self,
        address: &[u8],
    ) -> SysResult<Option<Arc<dyn KernelSocket>>> {
        let Ok(Some(path)) = crate::net::unix_pathname_from_raw(address) else {
            return Ok(None);
        };
        let (node, _) = self
            .services
            .lookup_node_with_identity(&self.process.fs, &path, true)?;
        if node.kind() != aether_vfs::NodeKind::Socket {
            return Err(SysErr::NotSock);
        }
        crate::net::unix_lookup_bound_socket(address)?
            .ok_or(SysErr::ConnRefused)
            .map(Some)
    }

    pub(crate) fn read_socket_address(
        &self,
        address: u64,
        address_len: usize,
    ) -> SysResult<Vec<u8>> {
        if address == 0 {
            return Err(SysErr::Fault);
        }
        self.syscall_read_user_exact_buffer(address, address_len)
    }

    fn read_optional_socket_address(
        &self,
        address: u64,
        address_len: usize,
    ) -> SysResult<Option<Vec<u8>>> {
        if address == 0 || address_len == 0 {
            Ok(None)
        } else {
            self.read_socket_address(address, address_len).map(Some)
        }
    }

    fn read_socket_message(&self, address: u64) -> SysResult<SocketMessage> {
        let header = LinuxMsghdr::read_from(self, address)?;
        if header.iov_len > IOV_MAX {
            return Err(SysErr::Inval);
        }

        let name = self.read_optional_socket_address(header.name, header.name_len as usize)?;
        let control = if header.control == 0 || header.control_len == 0 {
            Vec::new()
        } else {
            self.syscall_read_user_exact_buffer(header.control, header.control_len)?
        };
        let (rights, explicit_credentials) = self.parse_socket_send_control(&control)?;

        let segments = super::super::util::read_iovec_array(
            &self.process.task.address_space,
            header.iov,
            header.iov_len,
        )?;
        let total_len = segments.iter().try_fold(0usize, |total, segment| {
            total
                .checked_add(segment.len)
                .filter(|next| *next <= MAX_RW_COUNT)
                .ok_or(SysErr::Inval)
        })?;
        let mut data = Vec::with_capacity(total_len);
        for segment in segments {
            if segment.len == 0 {
                continue;
            }
            data.extend_from_slice(&self.read_user_buffer(segment.base, segment.len)?);
        }

        Ok(SocketMessage {
            name,
            data,
            control,
            rights,
            sender: SocketCredentials::new(
                self.process.identity.pid,
                self.process.credentials.uid,
                self.process.credentials.gid,
            ),
            explicit_credentials,
            msg_flags: header.flags,
        })
    }

    fn write_iovec_bytes(
        &mut self,
        segments: &[super::super::util::UserIoVec],
        bytes: &[u8],
    ) -> SysResult<()> {
        let mut written = 0usize;
        for segment in segments {
            if written >= bytes.len() {
                break;
            }
            let count = core::cmp::min(segment.len, bytes.len() - written);
            if count == 0 {
                continue;
            }
            self.write_user_buffer(segment.base, &bytes[written..written + count])?;
            written += count;
        }
        Ok(())
    }

    fn write_socket_receive_name(
        &mut self,
        address: u64,
        address_len: u64,
        received: &SocketReceive,
    ) -> SysResult<()> {
        if let Some(name) = &received.address
            && address != 0
            && address_len != 0
        {
            let count = core::cmp::min(address_len as usize, name.len());
            if count != 0 {
                self.write_user_buffer(address, &name[..count])?;
            }
        }
        Ok(())
    }

    fn write_socket_receive_control(
        &mut self,
        address: u64,
        address_len: usize,
        received: &SocketReceive,
        flags: u64,
    ) -> SysResult<(usize, u32)> {
        let mut msg_flags = 0u32;
        let mut control = Vec::new();

        if address == 0 || address_len == 0 {
            if !received.control.is_empty()
                || !received.rights.is_empty()
                || received.credentials.is_some()
            {
                msg_flags |= MSG_CTRUNC;
            }
            return Ok((0, msg_flags));
        }

        if !received.control.is_empty() {
            control.extend_from_slice(received.control.as_slice());
        }

        if !received.rights.is_empty() {
            let base_len = control.len();
            let available = address_len.saturating_sub(base_len);
            let mut delivered = 0usize;
            while delivered < received.rights.len() {
                let remaining = received.rights.len() - delivered;
                let mut fit = remaining;
                while fit != 0 && cmsg_space(fit * core::mem::size_of::<i32>()) > available {
                    fit -= 1;
                }
                if fit == 0 {
                    msg_flags |= MSG_CTRUNC;
                    break;
                }

                let cloexec = (flags & MSG_CMSG_CLOEXEC) != 0;
                let mut payload = Vec::with_capacity(fit * core::mem::size_of::<i32>());
                for descriptor in &received.rights[delivered..delivered + fit] {
                    let mut installed = descriptor.clone();
                    installed.cloexec = false;
                    let fd = self.process.files.insert(installed, 0);
                    payload.extend_from_slice(&(fd as i32).to_ne_bytes());
                }
                control.extend_from_slice(
                    serialize_cmsg(crate::net::SOL_SOCKET, SCM_RIGHTS, payload.as_slice())
                        .as_slice(),
                );
                if cloexec {
                    for fd_bytes in payload.chunks_exact(core::mem::size_of::<i32>()) {
                        let fd = i32::from_ne_bytes(fd_bytes.try_into().map_err(|_| SysErr::Fault)?)
                            as u32;
                        let _ = self.process.files.with_descriptor_mut(fd, |descriptor| {
                            descriptor.cloexec = true;
                        });
                    }
                }
                delivered += fit;
                if delivered < received.rights.len() {
                    msg_flags |= MSG_CTRUNC;
                    break;
                }
            }
        }

        if let Some(credentials) = received.credentials {
            let available = address_len.saturating_sub(control.len());
            let mut payload = Vec::with_capacity(12);
            payload.extend_from_slice(&(credentials.pid as i32).to_ne_bytes());
            payload.extend_from_slice(&credentials.uid.to_ne_bytes());
            payload.extend_from_slice(&credentials.gid.to_ne_bytes());
            if cmsg_space(payload.len()) <= available {
                control.extend_from_slice(
                    serialize_cmsg(crate::net::SOL_SOCKET, SCM_CREDENTIALS, payload.as_slice())
                        .as_slice(),
                );
            } else {
                msg_flags |= MSG_CTRUNC;
            }
        }

        let count = core::cmp::min(address_len, control.len());
        if count != 0 {
            self.write_user_buffer(address, &control[..count])?;
        }
        if count < control.len() {
            msg_flags |= MSG_CTRUNC;
        }
        Ok((count, msg_flags))
    }

    fn parse_socket_send_control(
        &self,
        control: &[u8],
    ) -> SysResult<(Vec<FileDescriptor>, Option<SocketCredentials>)> {
        let mut rights = Vec::new();
        let mut credentials = None;
        let mut offset = 0usize;
        let current = SocketCredentials::new(
            self.process.identity.pid,
            self.process.credentials.uid,
            self.process.credentials.gid,
        );

        while offset + cmsg_header_len() <= control.len() {
            let len = usize::from_ne_bytes(
                control[offset..offset + core::mem::size_of::<usize>()]
                    .try_into()
                    .map_err(|_| SysErr::Fault)?,
            );
            if len < cmsg_header_len() || offset + len > control.len() {
                return Err(SysErr::Inval);
            }

            let level_offset = offset + core::mem::size_of::<usize>();
            let level = i32::from_ne_bytes(
                control[level_offset..level_offset + 4]
                    .try_into()
                    .map_err(|_| SysErr::Fault)?,
            );
            let kind = i32::from_ne_bytes(
                control[level_offset + 4..level_offset + 8]
                    .try_into()
                    .map_err(|_| SysErr::Fault)?,
            );
            let payload = &control[offset + cmsg_header_len()..offset + len];

            if level == crate::net::SOL_SOCKET {
                match kind {
                    SCM_RIGHTS => {
                        if !rights.is_empty()
                            || payload.is_empty()
                            || !payload.len().is_multiple_of(core::mem::size_of::<i32>())
                        {
                            return Err(SysErr::Inval);
                        }
                        let count = payload.len() / core::mem::size_of::<i32>();
                        if count > SCM_MAX_FD {
                            return Err(SysErr::TooManyRefs);
                        }
                        for fd_bytes in payload.chunks_exact(core::mem::size_of::<i32>()) {
                            let fd =
                                i32::from_ne_bytes(fd_bytes.try_into().map_err(|_| SysErr::Fault)?);
                            if fd < 0 {
                                return Err(SysErr::BadFd);
                            }
                            let descriptor =
                                self.process.files.get(fd as u32).ok_or(SysErr::BadFd)?;
                            rights.push(descriptor);
                        }
                    }
                    SCM_CREDENTIALS => {
                        if credentials.is_some() || payload.len() != 12 {
                            return Err(SysErr::Inval);
                        }
                        let pid =
                            i32::from_ne_bytes(payload[..4].try_into().map_err(|_| SysErr::Fault)?);
                        if pid <= 0 {
                            return Err(SysErr::Inval);
                        }
                        let explicit = SocketCredentials::new(
                            pid as u32,
                            u32::from_ne_bytes(
                                payload[4..8].try_into().map_err(|_| SysErr::Fault)?,
                            ),
                            u32::from_ne_bytes(
                                payload[8..12].try_into().map_err(|_| SysErr::Fault)?,
                            ),
                        );
                        if !self.process.credentials.is_superuser() && explicit != current {
                            return Err(SysErr::Perm);
                        }
                        credentials = Some(explicit);
                    }
                    _ => {}
                }
            }

            let next = offset.saturating_add(cmsg_align(len));
            if next <= offset || next > control.len() {
                break;
            }
            offset = next;
        }

        Ok((rights, credentials))
    }

    fn write_socket_receive_address(
        &mut self,
        address: u64,
        address_len: u64,
        received: &SocketReceive,
    ) -> SysResult<()> {
        self.write_returned_socket_address(address, address_len, received.address.as_deref())
    }

    pub(crate) fn write_returned_socket_address(
        &mut self,
        address: u64,
        address_len: u64,
        returned: Option<&[u8]>,
    ) -> SysResult<()> {
        if address == 0 {
            return Ok(());
        }
        if address_len == 0 {
            return Err(SysErr::Fault);
        }

        let actual_len = returned.map(|name| name.len()).unwrap_or(0);
        let requested = u32::from_ne_bytes(
            self.syscall_read_user_exact_buffer(address_len, 4)?
                .as_slice()
                .try_into()
                .map_err(|_| SysErr::Fault)?,
        ) as usize;
        if let Some(name) = returned {
            let count = core::cmp::min(requested, name.len());
            if count != 0 {
                self.write_user_buffer(address, &name[..count])?;
            }
        }
        self.write_user_buffer(address_len, &(actual_len as u32).to_ne_bytes())
    }

    pub(crate) fn install_accepted_socket(
        &mut self,
        accepted: AcceptedSocket,
        address: u64,
        address_len: u64,
        flags: u64,
    ) -> SysResult<u64> {
        let (open_flags, cloexec) = parse_accept4_flags(flags)?;
        let node: aether_vfs::NodeRef =
            aether_vfs::FileNode::new("socket", Arc::new(SocketFile::new(accepted.socket)));
        let filesystem = super::super::util::anonymous_filesystem_identity();
        let fd = self
            .process
            .files
            .insert_node(node, open_flags, filesystem, None, cloexec) as u64;

        if address != 0 {
            self.write_returned_socket_address(address, address_len, accepted.address.as_deref())?;
        }

        Ok(fd)
    }

    pub(super) fn syscall_fadvise64(
        &mut self,
        fd: u64,
        offset: u64,
        len: u64,
        advice: u64,
    ) -> SysResult<u64> {
        let advice = match advice {
            0 => FileAdvice::Normal,
            1 => FileAdvice::Random,
            2 => FileAdvice::Sequential,
            3 => FileAdvice::WillNeed,
            4 => FileAdvice::DontNeed,
            5 => FileAdvice::NoReuse,
            _ => return Err(SysErr::Inval),
        };

        let descriptor = self.process.files.get(fd as u32).ok_or(SysErr::BadFd)?;
        let file = descriptor.file.lock();
        match file.node().kind() {
            NodeKind::File | NodeKind::BlockDevice => {}
            NodeKind::Fifo | NodeKind::Socket => return Err(SysErr::SPipe),
            _ => return Err(SysErr::Inval),
        }
        file.advise(offset, len, advice).map_err(SysErr::from)?;
        Ok(0)
    }

    pub(crate) fn fs_view_for_dirfd(&self, dirfd: i64, path: &str) -> SysResult<ProcessFsContext> {
        const AT_FDCWD: i64 = -100;

        if path.starts_with('/') || dirfd == AT_FDCWD {
            return Ok(self.process.fs.fork_copy());
        }

        let descriptor = self.process.files.get(dirfd as u32).ok_or(SysErr::BadFd)?;
        let location = descriptor.location.clone().ok_or(SysErr::NotDir)?;
        if location.node().kind() != NodeKind::Directory {
            return Err(SysErr::NotDir);
        }

        let mut fs = self.process.fs.fork_copy();
        fs.set_cwd_location(location);
        Ok(fs)
    }

    pub(crate) fn location_for_lookup(
        &self,
        fs: &ProcessFsContext,
        path: &str,
        node: &NodeRef,
    ) -> Option<FsLocation> {
        (node.kind() == NodeKind::Directory)
            .then(|| FsLocation::new(resolve_at_path(fs, path), node.clone()))
    }
}

fn parse_accept4_flags(flags: u64) -> SysResult<(aether_vfs::OpenFlags, bool)> {
    if (flags & !ACCEPT4_FLAGS_MASK) != 0 {
        return Err(SysErr::Inval);
    }

    let mut open_bits = aether_vfs::OpenFlags::READ | aether_vfs::OpenFlags::WRITE;
    if (flags & SOCK_NONBLOCK) != 0 {
        open_bits |= aether_vfs::OpenFlags::NONBLOCK;
    }

    Ok((
        aether_vfs::OpenFlags::from_bits(open_bits),
        (flags & SOCK_CLOEXEC) != 0,
    ))
}

fn linux_poll_to_events(events: i16) -> PollEvents {
    let mut poll_events = PollEvents::empty();
    if (events & (POLLIN | POLLPRI | POLLRDNORM | POLLRDBAND)) != 0 {
        poll_events = poll_events | PollEvents::READ;
    }
    if (events & (POLLOUT | POLLWRNORM | POLLWRBAND)) != 0 {
        poll_events = poll_events | PollEvents::WRITE;
    }
    if (events & POLLERR) != 0 {
        poll_events = poll_events | PollEvents::ERROR;
    }
    if (events & POLLHUP) != 0 {
        poll_events = poll_events | PollEvents::HUP;
    }
    if (events & POLLNVAL) != 0 {
        poll_events = poll_events | PollEvents::INVALID;
    }
    if (events & POLLRDHUP) != 0 {
        poll_events = poll_events | PollEvents::RDHUP;
    }
    poll_events
}

fn events_to_linux_poll(events: PollEvents) -> i16 {
    let mut result = 0i16;
    if events.contains(PollEvents::READ) {
        result |= POLLIN;
    }
    if events.contains(PollEvents::WRITE) {
        result |= POLLOUT;
    }
    if events.contains(PollEvents::ERROR) {
        result |= POLLERR;
    }
    if events.contains(PollEvents::HUP) {
        result |= POLLHUP;
    }
    if events.contains(PollEvents::INVALID) {
        result |= POLLNVAL;
    }
    if events.contains(PollEvents::RDHUP) {
        result |= POLLRDHUP;
    }
    result
}

fn fd_set_bytes_len(nfds: usize) -> usize {
    nfds.div_ceil(8)
}

fn fd_set_test(bits: &[u8], fd: usize) -> bool {
    let Some(byte) = bits.get(fd / 8) else {
        return false;
    };
    (*byte & (1u8 << (fd % 8))) != 0
}

fn fd_set_assign(bits: &mut [u8], fd: usize, value: bool) {
    let Some(byte) = bits.get_mut(fd / 8) else {
        return;
    };
    let mask = 1u8 << (fd % 8);
    if value {
        *byte |= mask;
    } else {
        *byte &= !mask;
    }
}

pub(crate) fn resolve_at_path(fs: &ProcessFsContext, path: &str) -> alloc::string::String {
    let mut components = if path.starts_with('/') {
        fs.root_path()
            .split('/')
            .filter(|component| !component.is_empty())
            .map(alloc::string::String::from)
            .collect::<alloc::vec::Vec<_>>()
    } else {
        fs.cwd_path()
            .split('/')
            .filter(|component| !component.is_empty())
            .map(alloc::string::String::from)
            .collect::<alloc::vec::Vec<_>>()
    };
    let anchor = fs
        .root_path()
        .split('/')
        .filter(|component| !component.is_empty())
        .count();
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                if components.len() > anchor {
                    let _ = components.pop();
                }
            }
            other => components.push(alloc::string::String::from(other)),
        }
    }
    if components.is_empty() {
        alloc::string::String::from("/")
    } else {
        alloc::format!("/{}", components.join("/"))
    }
}
