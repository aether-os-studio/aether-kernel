extern crate alloc;

use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::any::Any;
use core::cmp::min;
use core::ptr;
use core::sync::atomic::{AtomicU64, Ordering};

use aether_frame::libs::spin::SpinLock;
use aether_vfs::{FsError, FsResult, PollEvents, SharedWaitListener, WaitQueue};

use super::{
    AcceptedSocket, KernelSocket, SO_PASSCRED, SO_PEERCRED, SO_RCVBUF, SO_RCVBUFFORCE, SO_SNDBUF,
    SO_SNDBUFFORCE, SocketCredentials, SocketDomain, SocketMessage, SocketType, encode_sockopt_i32,
    register_socket_domain,
};
use crate::errno::{SysErr, SysResult};
use crate::fs::FileDescriptor;

const AF_UNIX: i32 = 1;
const SOCK_STREAM: u32 = 1;
const SOCK_DGRAM: u32 = 2;
const SOCK_SEQPACKET: u32 = 5;
const UNIX_BUFFER_SIZE: usize = 256 * 1024;
const MSG_PEEK: u64 = 0x0002;
const MSG_TRUNC: u32 = 0x0020;

static BOUND_SOCKETS: SpinLock<BTreeMap<UnixAddress, Weak<UnixSocket>>> =
    SpinLock::new(BTreeMap::new());

pub fn init() {
    register_socket_domain(&UNIX_SOCKET_DOMAIN);
}

static UNIX_SOCKET_DOMAIN: UnixSocketDomain = UnixSocketDomain;

struct UnixSocketDomain;

impl SocketDomain for UnixSocketDomain {
    fn domain(&self) -> i32 {
        AF_UNIX
    }

    fn create(
        &self,
        socket_type: SocketType,
        protocol: i32,
        owner: SocketCredentials,
    ) -> SysResult<Arc<dyn KernelSocket>> {
        if protocol != 0 {
            return Err(SysErr::ProtoNoSupport);
        }

        match socket_type.kind() {
            SOCK_STREAM | SOCK_DGRAM | SOCK_SEQPACKET => {
                Ok(UnixSocket::shared(socket_type.kind(), owner))
            }
            _ => Err(SysErr::ProtoNoSupport),
        }
    }

    fn create_pair(
        &self,
        socket_type: SocketType,
        protocol: i32,
        owner: SocketCredentials,
    ) -> SysResult<(Arc<dyn KernelSocket>, Arc<dyn KernelSocket>)> {
        if protocol != 0 {
            return Err(SysErr::ProtoNoSupport);
        }

        match socket_type.kind() {
            SOCK_STREAM | SOCK_DGRAM | SOCK_SEQPACKET => {
                let left = UnixSocket::shared(socket_type.kind(), owner);
                let right = UnixSocket::shared(socket_type.kind(), owner);
                UnixSocket::pair(&left, &right);
                Ok((left, right))
            }
            _ => Err(SysErr::ProtoNoSupport),
        }
    }
}

struct UnixSocket {
    kind: u32,
    owner: SocketCredentials,
    self_ref: SpinLock<Weak<UnixSocket>>,
    state: SpinLock<UnixSocketState>,
    version: AtomicU64,
    waiters: WaitQueue,
}

struct UnixSocketState {
    bound_address: Option<UnixAddress>,
    peer: Option<Weak<UnixSocket>>,
    peer_credentials: Option<SocketCredentials>,
    established: bool,
    listening: bool,
    backlog_limit: usize,
    backlog: VecDeque<Arc<UnixSocket>>,
    recv_stream: VecDeque<UnixStreamChunk>,
    recv_packets: VecDeque<UnixPacket>,
    recv_size: usize,
    passcred: bool,
    sndbuf: usize,
    rcvbuf: usize,
    shut_rd: bool,
    shut_wr: bool,
    closed: bool,
}

#[derive(Clone)]
struct UnixPacket {
    data: Vec<u8>,
    source: Option<Vec<u8>>,
    ancillary: Option<UnixAncillary>,
}

#[derive(Clone)]
struct UnixStreamChunk {
    data: Vec<u8>,
    read_offset: usize,
    ancillary: Option<UnixAncillary>,
}

#[derive(Clone)]
struct UnixAncillary {
    rights: Vec<FileDescriptor>,
    credentials: Option<SocketCredentials>,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct UnixAddress {
    key: Vec<u8>,
    abstract_namespace: bool,
}

impl UnixAddress {
    fn from_raw(address: &[u8]) -> SysResult<Self> {
        if address.len() < 2 {
            return Err(SysErr::Inval);
        }

        let family = u16::from_ne_bytes([address[0], address[1]]);
        if family != AF_UNIX as u16 {
            return Err(SysErr::AfNoSupport);
        }

        let path = &address[2..];
        if path.is_empty() {
            return Err(SysErr::Inval);
        }

        if path[0] == 0 {
            return Ok(Self {
                key: path.to_vec(),
                abstract_namespace: true,
            });
        }

        let end = path
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(path.len());
        if end == 0 {
            return Err(SysErr::Inval);
        }

        Ok(Self {
            key: path[..end].to_vec(),
            abstract_namespace: false,
        })
    }

    fn serialize(&self) -> Vec<u8> {
        let mut bytes =
            Vec::with_capacity(2 + self.key.len() + usize::from(!self.abstract_namespace));
        bytes.extend_from_slice(&(AF_UNIX as u16).to_ne_bytes());
        bytes.extend_from_slice(&self.key);
        if !self.abstract_namespace {
            bytes.push(0);
        }
        bytes
    }
}

pub fn pathname_from_raw(address: &[u8]) -> SysResult<Option<String>> {
    let address = UnixAddress::from_raw(address)?;
    if address.abstract_namespace {
        return Ok(None);
    }
    core::str::from_utf8(&address.key)
        .map(|path| Some(String::from(path)))
        .map_err(|_| SysErr::Inval)
}

impl UnixSocket {
    fn shared(kind: u32, owner: SocketCredentials) -> Arc<Self> {
        let socket = Arc::new(Self {
            kind,
            owner,
            self_ref: SpinLock::new(Weak::new()),
            state: SpinLock::new(UnixSocketState {
                bound_address: None,
                peer: None,
                peer_credentials: None,
                established: false,
                listening: false,
                backlog_limit: 0,
                backlog: VecDeque::new(),
                recv_stream: VecDeque::new(),
                recv_packets: VecDeque::new(),
                recv_size: 0,
                passcred: false,
                sndbuf: UNIX_BUFFER_SIZE,
                rcvbuf: UNIX_BUFFER_SIZE,
                shut_rd: false,
                shut_wr: false,
                closed: false,
            }),
            version: AtomicU64::new(1),
            waiters: WaitQueue::new(),
        });
        *socket.self_ref.lock_irqsave() = Arc::downgrade(&socket);
        socket
    }

    fn is_stream_like(&self) -> bool {
        self.kind == SOCK_STREAM
    }

    fn is_packet_like(&self) -> bool {
        self.kind == SOCK_DGRAM || self.kind == SOCK_SEQPACKET
    }

    fn unnamed_address() -> Vec<u8> {
        (AF_UNIX as u16).to_ne_bytes().to_vec()
    }

    fn pair(left: &Arc<Self>, right: &Arc<Self>) {
        {
            let mut left_state = left.state.lock_irqsave();
            left_state.peer = Some(Arc::downgrade(right));
            left_state.peer_credentials = Some(right.owner);
            left_state.established = true;
        }
        {
            let mut right_state = right.state.lock_irqsave();
            right_state.peer = Some(Arc::downgrade(left));
            right_state.peer_credentials = Some(left.owner);
            right_state.established = true;
        }
        left.notify(PollEvents::WRITE);
        right.notify(PollEvents::WRITE);
    }

    fn peer(&self) -> Option<Arc<UnixSocket>> {
        self.state
            .lock_irqsave()
            .peer
            .as_ref()
            .and_then(Weak::upgrade)
    }

    fn shared_self(&self) -> Option<Arc<UnixSocket>> {
        self.self_ref.lock_irqsave().upgrade()
    }

    fn local_name_locked(state: &UnixSocketState) -> Vec<u8> {
        state
            .bound_address
            .as_ref()
            .map(UnixAddress::serialize)
            .unwrap_or_else(Self::unnamed_address)
    }

    fn local_name(&self) -> Vec<u8> {
        Self::local_name_locked(&self.state.lock_irqsave())
    }

    fn peer_name_bytes(&self) -> SysResult<Vec<u8>> {
        let peer = self.peer().ok_or(SysErr::NotConn)?;
        Ok(Self::local_name_locked(&peer.state.lock_irqsave()))
    }

    fn bump(&self) {
        let _ = self.version.fetch_add(1, Ordering::AcqRel);
    }

    fn notify(&self, events: PollEvents) {
        self.bump();
        self.waiters.notify(events);
    }

    fn read_ready_locked(&self, state: &UnixSocketState) -> bool {
        if state.listening {
            return !state.backlog.is_empty();
        }
        if state.shut_rd {
            return true;
        }
        if self.is_stream_like() {
            !state.recv_stream.is_empty() || self.stream_peer_eof_locked(state)
        } else {
            !state.recv_packets.is_empty()
        }
    }

    fn write_ready_locked(&self, state: &UnixSocketState) -> bool {
        if state.shut_wr || state.listening {
            return false;
        }
        if self.is_packet_like() {
            return true;
        }
        let Some(peer) = state.peer.as_ref().and_then(Weak::upgrade) else {
            return false;
        };
        let peer_state = peer.state.lock_irqsave();
        !peer_state.shut_rd && peer_state.recv_size < peer_state.rcvbuf
    }

    fn stream_peer_eof_locked(&self, state: &UnixSocketState) -> bool {
        if !state.established {
            return false;
        }

        match state.peer.as_ref().and_then(Weak::upgrade) {
            Some(peer) => peer.state.lock_irqsave().shut_wr,
            None => true,
        }
    }

    fn enqueue_stream(
        peer: &Arc<UnixSocket>,
        buffer: &[u8],
        ancillary: Option<UnixAncillary>,
    ) -> SysResult<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let mut state = peer.state.lock_irqsave();
        if state.shut_rd {
            return Err(SysErr::Pipe);
        }
        let space = state.rcvbuf.saturating_sub(state.recv_size);
        if space == 0 {
            return Err(SysErr::Again);
        }
        let count = min(space, buffer.len());
        state.recv_stream.push_back(UnixStreamChunk {
            data: buffer[..count].to_vec(),
            read_offset: 0,
            ancillary,
        });
        state.recv_size = state.recv_size.saturating_add(count);
        drop(state);
        peer.notify(PollEvents::READ);
        Ok(count)
    }

    fn enqueue_packet(peer: &Arc<UnixSocket>, buffer: &[u8]) -> SysResult<usize> {
        Self::enqueue_packet_from(peer, buffer, None, None)
    }

    fn enqueue_packet_from(
        peer: &Arc<UnixSocket>,
        buffer: &[u8],
        source: Option<Vec<u8>>,
        ancillary: Option<UnixAncillary>,
    ) -> SysResult<usize> {
        let mut state = peer.state.lock_irqsave();
        if state.shut_rd {
            return Err(SysErr::Pipe);
        }
        let next = state.recv_size.saturating_add(buffer.len());
        if next > state.rcvbuf {
            return Err(SysErr::Again);
        }
        state.recv_packets.push_back(UnixPacket {
            data: buffer.to_vec(),
            source,
            ancillary,
        });
        state.recv_size = next;
        drop(state);
        peer.notify(PollEvents::READ);
        Ok(buffer.len())
    }

    fn peer_credentials(&self) -> Option<SocketCredentials> {
        self.state.lock_irqsave().peer_credentials
    }

    fn serialize_peer_credentials(&self) -> SysResult<Vec<u8>> {
        let credentials = self.peer_credentials().ok_or(SysErr::NotConn)?;
        let mut bytes = Vec::with_capacity(12);
        bytes.extend_from_slice(&(credentials.pid as i32).to_ne_bytes());
        bytes.extend_from_slice(&credentials.uid.to_ne_bytes());
        bytes.extend_from_slice(&credentials.gid.to_ne_bytes());
        Ok(bytes)
    }

    fn receive_from_ancillary(
        address: Option<Vec<u8>>,
        ancillary: Option<UnixAncillary>,
        msg_flags: u32,
        bytes_read: usize,
    ) -> super::SocketReceive {
        let (rights, credentials) = ancillary
            .map(|ancillary| (ancillary.rights, ancillary.credentials))
            .unwrap_or_else(|| (Vec::new(), None));
        super::SocketReceive {
            address,
            control: Vec::new(),
            rights,
            credentials,
            msg_flags,
            bytes_read,
        }
    }

    fn build_outbound_ancillary(message: &SocketMessage, passcred: bool) -> Option<UnixAncillary> {
        let credentials = message
            .explicit_credentials
            .or_else(|| passcred.then_some(message.sender));
        if message.rights.is_empty() && credentials.is_none() {
            return None;
        }
        Some(UnixAncillary {
            rights: message.rights.clone(),
            credentials,
        })
    }

    fn recv_stream(
        &self,
        buffer: &mut [u8],
        flags: u64,
        include_control: bool,
    ) -> SysResult<super::SocketReceive> {
        let peek = (flags & MSG_PEEK) != 0;
        let mut state = self.state.lock_irqsave();
        if state.shut_rd {
            return Ok(Self::receive_from_ancillary(None, None, 0, 0));
        }
        if state.recv_stream.is_empty() {
            if !state.established {
                return Err(SysErr::NotConn);
            }
            if self.stream_peer_eof_locked(&state) {
                return Ok(Self::receive_from_ancillary(None, None, 0, 0));
            }
            return Err(SysErr::Again);
        }

        let mut written = 0usize;
        let mut received_ancillary = None;

        if peek {
            for chunk in state.recv_stream.iter() {
                if written >= buffer.len() {
                    break;
                }
                let available = &chunk.data[chunk.read_offset..];
                if available.is_empty() {
                    continue;
                }
                if include_control && received_ancillary.is_none() {
                    received_ancillary = chunk.ancillary.clone();
                }
                let count = min(buffer.len() - written, available.len());
                buffer[written..written + count].copy_from_slice(&available[..count]);
                written += count;
            }
            return Ok(Self::receive_from_ancillary(
                None,
                received_ancillary,
                0,
                written,
            ));
        }

        while written < buffer.len() {
            let remove_front;
            let mut consumed = 0usize;
            {
                let Some(chunk) = state.recv_stream.front_mut() else {
                    break;
                };
                let available = chunk.data.len().saturating_sub(chunk.read_offset);
                if available == 0 {
                    remove_front = true;
                } else {
                    if include_control {
                        if received_ancillary.is_none() {
                            received_ancillary = chunk.ancillary.take();
                        } else {
                            chunk.ancillary = None;
                        }
                    } else {
                        chunk.ancillary = None;
                    }
                    let count = min(buffer.len() - written, available);
                    buffer[written..written + count]
                        .copy_from_slice(&chunk.data[chunk.read_offset..chunk.read_offset + count]);
                    chunk.read_offset += count;
                    consumed = count;
                    written += count;
                    remove_front = chunk.read_offset == chunk.data.len();
                }
            }
            state.recv_size = state.recv_size.saturating_sub(consumed);
            if remove_front {
                let _ = state.recv_stream.pop_front();
            }
        }

        let peer = state.peer.as_ref().and_then(Weak::upgrade);
        drop(state);
        if let Some(peer) = peer {
            peer.notify(PollEvents::WRITE);
        }
        Ok(Self::receive_from_ancillary(
            None,
            received_ancillary,
            0,
            written,
        ))
    }

    fn recv_packet(
        &self,
        buffer: &mut [u8],
        flags: u64,
        include_control: bool,
    ) -> SysResult<super::SocketReceive> {
        let peek = (flags & MSG_PEEK) != 0;
        let mut state = self.state.lock_irqsave();
        if state.shut_rd {
            return Ok(Self::receive_from_ancillary(None, None, 0, 0));
        }
        let packet = if peek {
            state.recv_packets.front().cloned()
        } else {
            state.recv_packets.pop_front()
        };
        let Some(packet) = packet else {
            return Err(SysErr::Again);
        };

        let count = min(buffer.len(), packet.data.len());
        buffer[..count].copy_from_slice(&packet.data[..count]);
        if !peek {
            state.recv_size = state.recv_size.saturating_sub(packet.data.len());
        }
        let peer = state.peer.as_ref().and_then(Weak::upgrade);
        drop(state);
        if !peek && let Some(peer) = peer {
            peer.notify(PollEvents::WRITE);
        }

        let ancillary = include_control.then_some(packet.ancillary).flatten();
        Ok(Self::receive_from_ancillary(
            packet.source,
            ancillary,
            if packet.data.len() > count {
                MSG_TRUNC
            } else {
                0
            },
            count,
        ))
    }

    fn lookup_bound(address: &UnixAddress) -> Option<Arc<UnixSocket>> {
        let mut bound = BOUND_SOCKETS.lock_irqsave();
        let socket = bound.get(address).and_then(Weak::upgrade);
        if socket.is_none() {
            let _ = bound.remove(address);
        }
        socket
    }
}

impl Drop for UnixSocket {
    fn drop(&mut self) {
        let bound_address = self.state.lock_irqsave().bound_address.clone();
        if let Some(address) = bound_address {
            let mut bound = BOUND_SOCKETS.lock_irqsave();
            let remove = bound
                .get(&address)
                .map(|socket| ptr::eq(socket.as_ptr(), self))
                .unwrap_or(false);
            if remove {
                let _ = bound.remove(&address);
            }
        }

        if let Some(peer) = self
            .state
            .lock_irqsave()
            .peer
            .as_ref()
            .and_then(Weak::upgrade)
        {
            peer.notify(PollEvents::READ | PollEvents::WRITE | PollEvents::ERROR);
        }
    }
}

impl KernelSocket for UnixSocket {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn domain(&self) -> i32 {
        AF_UNIX
    }

    fn socket_type(&self) -> u32 {
        self.kind
    }

    fn read(&self, buffer: &mut [u8]) -> FsResult<usize> {
        self.recv_from(buffer, 0)
            .map(|received| received.bytes_read)
            .map_err(|error| match error {
                SysErr::Again => FsError::WouldBlock,
                SysErr::NotConn => FsError::WouldBlock,
                _ => FsError::Unsupported,
            })
    }

    fn write(&self, buffer: &[u8]) -> FsResult<usize> {
        self.send_to(buffer, None, 0).map_err(|error| match error {
            SysErr::Again => FsError::WouldBlock,
            SysErr::Pipe => FsError::BrokenPipe,
            _ => FsError::Unsupported,
        })
    }

    fn recv_from(&self, buffer: &mut [u8], flags: u64) -> SysResult<super::SocketReceive> {
        if self.is_stream_like() {
            self.recv_stream(buffer, flags, false)
        } else {
            self.recv_packet(buffer, flags, false)
        }
    }

    fn recv_msg(&self, buffer: &mut [u8], flags: u64) -> SysResult<super::SocketReceive> {
        if self.is_stream_like() {
            self.recv_stream(buffer, flags, true)
        } else {
            self.recv_packet(buffer, flags, true)
        }
    }

    fn connect(&self, address: &[u8]) -> SysResult<()> {
        let address = UnixAddress::from_raw(address)?;
        let target = Self::lookup_bound(&address).ok_or(SysErr::NoEnt)?;

        if self.kind == SOCK_DGRAM {
            let mut state = self.state.lock_irqsave();
            state.peer = Some(Arc::downgrade(&target));
            state.peer_credentials = Some(target.owner);
            state.established = true;
            drop(state);
            self.notify(PollEvents::WRITE);
            return Ok(());
        }

        {
            let state = target.state.lock_irqsave();
            if !state.listening {
                return Err(SysErr::ConnRefused);
            }
        }

        let server = UnixSocket::shared(self.kind, target.owner);
        {
            let mut server_state = server.state.lock_irqsave();
            server_state.bound_address = target.state.lock_irqsave().bound_address.clone();
            server_state.peer_credentials = Some(self.owner);
            server_state.established = true;
        }
        {
            let mut client_state = self.state.lock_irqsave();
            if client_state.peer.as_ref().and_then(Weak::upgrade).is_some() {
                return Err(SysErr::IsConn);
            }
            client_state.peer = Some(Arc::downgrade(&server));
            client_state.peer_credentials = Some(server.owner);
            client_state.established = true;
        }
        {
            let mut listener_state = target.state.lock_irqsave();
            let backlog_limit = listener_state.backlog_limit.max(1);
            if listener_state.backlog.len() >= backlog_limit {
                return Err(SysErr::Again);
            }
            let client = self.shared_self().ok_or(SysErr::ConnRefused)?;
            {
                let mut server_state = server.state.lock_irqsave();
                server_state.peer = Some(Arc::downgrade(&client));
            }
            listener_state.backlog.push_back(server);
        }

        target.notify(PollEvents::READ);
        self.notify(PollEvents::WRITE);
        Ok(())
    }

    fn bind(&self, address: &[u8]) -> SysResult<()> {
        let address = UnixAddress::from_raw(address)?;
        let state = self.state.lock_irqsave();
        if state.bound_address.is_some() {
            return Err(SysErr::Inval);
        }
        drop(state);

        let mut bound = BOUND_SOCKETS.lock_irqsave();
        let this = self.shared_self().ok_or(SysErr::AddrInUse)?;
        if let Some(existing) = bound.get(&address).and_then(Weak::upgrade)
            && !Arc::ptr_eq(&existing, &this)
        {
            return Err(SysErr::AddrInUse);
        }
        // TODO: Pathname AF_UNIX sockets should also create a filesystem socket inode and
        // participate in VFS lifetime rules. The current implementation keeps a kernel-only
        // bind table so connect()/sendto() can resolve the address correctly.
        bound.insert(address.clone(), Arc::downgrade(&this));
        let mut state = self.state.lock_irqsave();
        state.bound_address = Some(address);
        Ok(())
    }

    fn listen(&self, backlog: i32) -> SysResult<()> {
        if self.kind != SOCK_STREAM && self.kind != SOCK_SEQPACKET {
            return Err(SysErr::NoSys);
        }
        let mut state = self.state.lock_irqsave();
        state.listening = true;
        state.backlog_limit = backlog.max(1) as usize;
        state.backlog.clear();
        drop(state);
        self.notify(PollEvents::WRITE);
        Ok(())
    }

    fn accept(&self) -> SysResult<AcceptedSocket> {
        let mut state = self.state.lock_irqsave();
        if !state.listening {
            return Err(SysErr::Inval);
        }
        let accepted = state.backlog.pop_front().ok_or(SysErr::Again)?;
        let address = accepted
            .state
            .lock_irqsave()
            .peer
            .as_ref()
            .and_then(Weak::upgrade)
            .map(|peer| Self::local_name_locked(&peer.state.lock_irqsave()));
        drop(state);
        self.notify(PollEvents::WRITE);
        Ok(AcceptedSocket {
            socket: accepted,
            address,
        })
    }

    fn send_to(&self, buffer: &[u8], address: Option<&[u8]>, _flags: u64) -> SysResult<usize> {
        if buffer.is_empty() && self.is_stream_like() {
            return Ok(0);
        }
        let peer = if let Some(address) = address {
            let address = UnixAddress::from_raw(address)?;
            Self::lookup_bound(&address).ok_or(SysErr::NoEnt)?
        } else {
            self.peer().ok_or(if self.kind == SOCK_DGRAM {
                SysErr::DestAddrReq
            } else {
                SysErr::NotConn
            })?
        };

        if self.is_stream_like() {
            Self::enqueue_stream(&peer, buffer, None)
        } else {
            let source = self
                .state
                .lock_irqsave()
                .bound_address
                .as_ref()
                .map(UnixAddress::serialize);
            Self::enqueue_packet_from(&peer, buffer, source, None)
        }
    }

    fn send_msg(&self, message: &SocketMessage, _flags: u64) -> SysResult<usize> {
        let peer = if let Some(address) = message.name.as_deref() {
            let address = UnixAddress::from_raw(address)?;
            Self::lookup_bound(&address).ok_or(SysErr::NoEnt)?
        } else {
            self.peer().ok_or(if self.kind == SOCK_DGRAM {
                SysErr::DestAddrReq
            } else {
                SysErr::NotConn
            })?
        };

        let passcred = peer.state.lock_irqsave().passcred;
        let ancillary = Self::build_outbound_ancillary(message, passcred);
        if ancillary.is_some() && message.data.is_empty() {
            return Err(SysErr::Inval);
        }

        if self.is_stream_like() {
            Self::enqueue_stream(&peer, message.data.as_slice(), ancillary)
        } else {
            let source = self
                .state
                .lock_irqsave()
                .bound_address
                .as_ref()
                .map(UnixAddress::serialize);
            Self::enqueue_packet_from(&peer, message.data.as_slice(), source, ancillary)
        }
    }

    fn shutdown(&self, how: i32) -> SysResult<()> {
        let peer = {
            let mut state = self.state.lock_irqsave();
            if !state.established && state.peer.as_ref().and_then(Weak::upgrade).is_none() {
                return Err(SysErr::NotConn);
            }

            match how {
                0 => state.shut_rd = true,
                1 => state.shut_wr = true,
                2 => {
                    state.shut_rd = true;
                    state.shut_wr = true;
                }
                _ => return Err(SysErr::Inval),
            }

            state.peer.as_ref().and_then(Weak::upgrade)
        };

        self.notify(PollEvents::READ | PollEvents::WRITE | PollEvents::ERROR);
        if let Some(peer) = peer {
            peer.notify(PollEvents::READ | PollEvents::WRITE | PollEvents::ERROR);
        }
        Ok(())
    }

    fn sock_name(&self) -> SysResult<Vec<u8>> {
        Ok(self.local_name())
    }

    fn peer_name(&self) -> SysResult<Vec<u8>> {
        self.peer_name_bytes()
    }

    fn is_listening(&self) -> bool {
        self.state.lock_irqsave().listening
    }

    fn poll(&self, events: PollEvents) -> FsResult<PollEvents> {
        let state = self.state.lock_irqsave();
        let mut ready = PollEvents::empty();
        let stream_peer_eof = self.is_stream_like() && self.stream_peer_eof_locked(&state);

        if events.contains(PollEvents::READ) && self.read_ready_locked(&state) {
            ready = ready | PollEvents::READ;
        }
        if events.contains(PollEvents::WRITE) && self.write_ready_locked(&state) {
            ready = ready | PollEvents::WRITE;
        }
        if state.shut_wr || stream_peer_eof {
            ready = ready | PollEvents::ERROR;
        }

        Ok(ready)
    }

    fn register_waiter(
        &self,
        events: PollEvents,
        listener: SharedWaitListener,
    ) -> FsResult<Option<u64>> {
        Ok(Some(self.waiters.register(events, listener)))
    }

    fn unregister_waiter(&self, waiter_id: u64) -> FsResult<()> {
        let _ = self.waiters.unregister(waiter_id);
        Ok(())
    }

    fn setsockopt(&self, level: i32, optname: i32, value: &[u8]) -> SysResult<()> {
        if level != super::SOL_SOCKET {
            return Err(SysErr::NoProtoOpt);
        }
        if value.len() < core::mem::size_of::<i32>() {
            return Err(SysErr::Inval);
        }
        let parsed = i32::from_ne_bytes(
            value[..core::mem::size_of::<i32>()]
                .try_into()
                .map_err(|_| SysErr::Fault)?,
        );
        let mut state = self.state.lock_irqsave();
        match optname {
            SO_PASSCRED => {
                state.passcred = parsed != 0;
                Ok(())
            }
            SO_SNDBUF | SO_SNDBUFFORCE => {
                state.sndbuf = normalize_socket_buffer(parsed);
                Ok(())
            }
            SO_RCVBUF | SO_RCVBUFFORCE => {
                state.rcvbuf = normalize_socket_buffer(parsed);
                Ok(())
            }
            _ => Err(SysErr::NoProtoOpt),
        }
    }

    fn getsockopt(&self, level: i32, optname: i32) -> SysResult<Vec<u8>> {
        if level != super::SOL_SOCKET {
            return Err(SysErr::NoProtoOpt);
        }
        let state = self.state.lock_irqsave();
        match optname {
            SO_PASSCRED => Ok(encode_sockopt_i32(state.passcred as i32)),
            SO_SNDBUF | SO_SNDBUFFORCE => Ok(encode_sockopt_i32(clamp_sockopt_i32(state.sndbuf))),
            SO_RCVBUF | SO_RCVBUFFORCE => Ok(encode_sockopt_i32(clamp_sockopt_i32(state.rcvbuf))),
            SO_PEERCRED => {
                drop(state);
                self.serialize_peer_credentials()
            }
            _ => {
                drop(state);
                self.getsockopt_sol_socket(optname)
            }
        }
    }
}

fn normalize_socket_buffer(value: i32) -> usize {
    value.max(4096).try_into().unwrap_or(UNIX_BUFFER_SIZE)
}

fn clamp_sockopt_i32(value: usize) -> i32 {
    value.min(i32::MAX as usize) as i32
}
