extern crate alloc;

use alloc::collections::VecDeque;
use alloc::sync::{Arc, Weak};
use alloc::vec;
use alloc::vec::Vec;
use core::any::Any;
use core::cmp::min;
use core::sync::atomic::{AtomicU64, Ordering};

use aether_frame::libs::spin::SpinLock;
use aether_vfs::{FsError, FsResult, PollEvents, SharedWaitListener, WaitQueue};

use super::{
    KernelSocket, SO_PASSCRED, SO_RCVBUF, SO_RCVBUFFORCE, SO_SNDBUF, SO_SNDBUFFORCE, SocketDomain,
    SocketType, encode_sockopt_i32, register_socket_domain,
};
use crate::errno::{SysErr, SysResult};

const AF_NETLINK: i32 = 16;
const NETLINK_KOBJECT_UEVENT: i32 = 15;
const NETLINK_KOBJECT_UEVENT_GROUP: u32 = 1;

const SOCK_DGRAM: u32 = 2;
const SOCK_RAW: u32 = 3;

const MSG_PEEK: u64 = 0x0002;
const MSG_TRUNC: u32 = 0x0020;

const SOL_NETLINK: i32 = 270;

const NETLINK_ADD_MEMBERSHIP: i32 = 1;
const NETLINK_DROP_MEMBERSHIP: i32 = 2;

const NETLINK_SOCKADDR_LEN: usize = 12;
const DEFAULT_SOCKET_BUFFER: usize = 256 * 1024;
const MIN_SOCKET_BUFFER: usize = 4096;

static SOCKETS: SpinLock<Vec<Weak<NetlinkSocket>>> = SpinLock::new(Vec::new());
static UEVENT_SEQNUM: AtomicU64 = AtomicU64::new(0);

pub fn init() {
    register_socket_domain(&NETLINK_SOCKET_DOMAIN);
}

pub fn reserve_kobject_uevent_seqnum() -> u64 {
    UEVENT_SEQNUM
        .fetch_add(1, Ordering::AcqRel)
        .saturating_add(1)
}

pub fn current_kobject_uevent_seqnum() -> u64 {
    UEVENT_SEQNUM.load(Ordering::Acquire)
}

pub fn publish_kobject_uevent(payload: &[u8]) {
    broadcast_message(
        NETLINK_KOBJECT_UEVENT,
        0,
        1u32 << (NETLINK_KOBJECT_UEVENT_GROUP - 1),
        payload,
    );
}

static NETLINK_SOCKET_DOMAIN: NetlinkSocketDomain = NetlinkSocketDomain;

struct NetlinkSocketDomain;

impl SocketDomain for NetlinkSocketDomain {
    fn domain(&self) -> i32 {
        AF_NETLINK
    }

    fn create(
        &self,
        socket_type: SocketType,
        protocol: i32,
        owner: super::SocketCredentials,
    ) -> SysResult<Arc<dyn KernelSocket>> {
        if protocol != NETLINK_KOBJECT_UEVENT {
            // TODO: add NETLINK_ROUTE/GENERIC families when the corresponding kernel
            // subsystems exist.
            return Err(SysErr::ProtoNoSupport);
        }

        match socket_type.kind() {
            SOCK_DGRAM | SOCK_RAW => Ok(NetlinkSocket::shared(
                protocol,
                socket_type.kind(),
                owner.pid,
            )),
            _ => Err(SysErr::SockTNoSupport),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SockAddrNetlink {
    portid: u32,
    groups: u32,
}

impl SockAddrNetlink {
    fn parse(address: &[u8]) -> SysResult<Self> {
        if address.len() < NETLINK_SOCKADDR_LEN {
            return Err(SysErr::Inval);
        }
        let family = u16::from_ne_bytes([address[0], address[1]]);
        if family != AF_NETLINK as u16 {
            return Err(SysErr::AfNoSupport);
        }
        Ok(Self {
            portid: u32::from_ne_bytes(address[4..8].try_into().map_err(|_| SysErr::Fault)?),
            groups: u32::from_ne_bytes(address[8..12].try_into().map_err(|_| SysErr::Fault)?),
        })
    }

    fn serialize(self) -> Vec<u8> {
        let mut bytes = vec![0u8; NETLINK_SOCKADDR_LEN];
        bytes[..2].copy_from_slice(&(AF_NETLINK as u16).to_ne_bytes());
        bytes[4..8].copy_from_slice(&self.portid.to_ne_bytes());
        bytes[8..12].copy_from_slice(&self.groups.to_ne_bytes());
        bytes
    }
}

#[derive(Clone)]
struct NetlinkMessage {
    data: Vec<u8>,
    sender_portid: u32,
    sender_groups: u32,
}

struct NetlinkState {
    portid: u32,
    groups: u32,
    connected: Option<SockAddrNetlink>,
    recv_queue: VecDeque<NetlinkMessage>,
    recv_bytes: usize,
    passcred: bool,
    sndbuf: usize,
    rcvbuf: usize,
}

struct NetlinkSocket {
    protocol: i32,
    kind: u32,
    owner_pid: u32,
    state: SpinLock<NetlinkState>,
    waiters: WaitQueue,
}

impl NetlinkSocket {
    fn shared(protocol: i32, kind: u32, owner_pid: u32) -> Arc<Self> {
        let socket = Arc::new(Self {
            protocol,
            kind,
            owner_pid,
            state: SpinLock::new(NetlinkState {
                portid: 0,
                groups: 0,
                connected: None,
                recv_queue: VecDeque::new(),
                recv_bytes: 0,
                passcred: false,
                sndbuf: DEFAULT_SOCKET_BUFFER,
                rcvbuf: DEFAULT_SOCKET_BUFFER,
            }),
            waiters: WaitQueue::new(),
        });
        SOCKETS.lock().push(Arc::downgrade(&socket));
        socket
    }

    fn ensure_portid(&self) -> u32 {
        let mut state = self.state.lock();
        if state.portid == 0 {
            state.portid = allocate_portid(self.protocol, self.owner_pid, self);
        }
        state.portid
    }

    fn local_address(&self) -> SockAddrNetlink {
        let state = self.state.lock();
        SockAddrNetlink {
            portid: state.portid,
            groups: state.groups,
        }
    }

    fn peer_address(&self) -> SysResult<SockAddrNetlink> {
        self.state.lock().connected.ok_or(SysErr::NotConn)
    }

    fn enqueue(&self, message: NetlinkMessage) -> SysResult<()> {
        {
            let mut state = self.state.lock();
            let next = state.recv_bytes.saturating_add(message.data.len());
            if next > state.rcvbuf {
                return Err(SysErr::NoBufs);
            }
            state.recv_bytes = next;
            state.recv_queue.push_back(message);
        }
        self.waiters.notify(PollEvents::READ);
        Ok(())
    }

    fn socket_options(&self, level: i32, optname: i32, value: &[u8]) -> SysResult<()> {
        match level {
            super::SOL_SOCKET => self.set_sol_socket(optname, value),
            SOL_NETLINK => self.set_sol_netlink(optname, value),
            _ => Err(SysErr::NoProtoOpt),
        }
    }

    fn set_sol_socket(&self, optname: i32, value: &[u8]) -> SysResult<()> {
        let parsed = read_sockopt_i32(value)?;
        let mut state = self.state.lock();
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

    fn set_sol_netlink(&self, optname: i32, value: &[u8]) -> SysResult<()> {
        let group = read_sockopt_u32(value)?;
        if group == 0 || group > 32 {
            return Err(SysErr::Inval);
        }

        let bit = 1u32 << (group - 1);
        let mut state = self.state.lock();
        match optname {
            NETLINK_ADD_MEMBERSHIP => {
                state.groups |= bit;
                Ok(())
            }
            NETLINK_DROP_MEMBERSHIP => {
                state.groups &= !bit;
                Ok(())
            }
            _ => Err(SysErr::NoProtoOpt),
        }
    }
}

impl KernelSocket for NetlinkSocket {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn domain(&self) -> i32 {
        AF_NETLINK
    }

    fn socket_type(&self) -> u32 {
        self.kind
    }

    fn protocol(&self) -> i32 {
        self.protocol
    }

    fn read(&self, buffer: &mut [u8]) -> FsResult<usize> {
        match self.recv_from(buffer, 0) {
            Ok(received) => Ok(received.bytes_read),
            Err(SysErr::Again) => Err(FsError::WouldBlock),
            Err(_) => Err(FsError::Unsupported),
        }
    }

    fn write(&self, buffer: &[u8]) -> FsResult<usize> {
        match self.send_to(buffer, None, 0) {
            Ok(written) => Ok(written),
            Err(SysErr::Again) => Err(FsError::WouldBlock),
            Err(SysErr::Pipe) => Err(FsError::BrokenPipe),
            Err(_) => Err(FsError::Unsupported),
        }
    }

    fn recv_from(&self, buffer: &mut [u8], flags: u64) -> SysResult<super::SocketReceive> {
        let peek = (flags & MSG_PEEK) != 0;
        let mut state = self.state.lock();
        let message = if peek {
            state.recv_queue.front().cloned()
        } else {
            state.recv_queue.pop_front()
        };
        let Some(message) = message else {
            return Err(SysErr::Again);
        };

        let count = min(buffer.len(), message.data.len());
        buffer[..count].copy_from_slice(&message.data[..count]);
        if !peek {
            state.recv_bytes = state.recv_bytes.saturating_sub(message.data.len());
        }
        let msg_flags = if message.data.len() > count {
            MSG_TRUNC
        } else {
            0
        };

        Ok(super::SocketReceive {
            address: Some(
                SockAddrNetlink {
                    portid: message.sender_portid,
                    groups: message.sender_groups,
                }
                .serialize(),
            ),
            control: Vec::new(),
            rights: Vec::new(),
            credentials: None,
            msg_flags,
            bytes_read: count,
        })
    }

    fn sock_name(&self) -> SysResult<Vec<u8>> {
        Ok(self.local_address().serialize())
    }

    fn peer_name(&self) -> SysResult<Vec<u8>> {
        Ok(self.peer_address()?.serialize())
    }

    fn connect(&self, address: &[u8]) -> SysResult<()> {
        let address = SockAddrNetlink::parse(address)?;
        if address.groups != 0 {
            return Err(SysErr::Inval);
        }
        let mut state = self.state.lock();
        state.connected = Some(address);
        if state.portid == 0 {
            state.portid = allocate_portid(self.protocol, self.owner_pid, self);
        }
        Ok(())
    }

    fn bind(&self, address: &[u8]) -> SysResult<()> {
        let address = SockAddrNetlink::parse(address)?;
        let portid = if address.portid == 0 {
            allocate_portid(self.protocol, self.owner_pid, self)
        } else {
            if portid_in_use(self.protocol, address.portid, self) {
                return Err(SysErr::AddrInUse);
            }
            address.portid
        };

        let mut state = self.state.lock();
        state.portid = portid;
        state.groups = address.groups;
        Ok(())
    }

    fn send_to(&self, buffer: &[u8], address: Option<&[u8]>, _flags: u64) -> SysResult<usize> {
        let destination = match address {
            Some(address) => SockAddrNetlink::parse(address)?,
            None => self.state.lock().connected.ok_or(SysErr::DestAddrReq)?,
        };

        let sender_portid = self.ensure_portid();
        if destination.groups != 0 {
            broadcast_message(self.protocol, sender_portid, destination.groups, buffer);
            return Ok(buffer.len());
        }
        if destination.portid == 0 {
            return Ok(buffer.len());
        }

        let Some(peer) = lookup_portid(self.protocol, destination.portid, self) else {
            return Err(SysErr::ConnRefused);
        };
        peer.enqueue(NetlinkMessage {
            data: buffer.to_vec(),
            sender_portid,
            sender_groups: 0,
        })?;
        Ok(buffer.len())
    }

    fn poll(&self, events: PollEvents) -> FsResult<PollEvents> {
        let state = self.state.lock();
        let mut ready = PollEvents::empty();
        if events.contains(PollEvents::READ) && !state.recv_queue.is_empty() {
            ready = ready | PollEvents::READ;
        }
        if events.contains(PollEvents::WRITE) {
            ready = ready | PollEvents::WRITE;
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
        self.socket_options(level, optname, value)
    }

    fn getsockopt(&self, level: i32, optname: i32) -> SysResult<Vec<u8>> {
        match level {
            super::SOL_SOCKET => {
                let state = self.state.lock();
                match optname {
                    SO_PASSCRED => Ok(encode_sockopt_i32(state.passcred as i32)),
                    SO_SNDBUF | SO_SNDBUFFORCE => {
                        Ok(encode_sockopt_i32(clamp_sockopt_i32(state.sndbuf)))
                    }
                    SO_RCVBUF | SO_RCVBUFFORCE => {
                        Ok(encode_sockopt_i32(clamp_sockopt_i32(state.rcvbuf)))
                    }
                    _ => {
                        drop(state);
                        self.getsockopt_sol_socket(optname)
                    }
                }
            }
            _ => Err(SysErr::NoProtoOpt),
        }
    }
}

fn live_sockets() -> Vec<Arc<NetlinkSocket>> {
    let mut sockets = SOCKETS.lock();
    let mut live = Vec::with_capacity(sockets.len());
    sockets.retain(|entry| match entry.upgrade() {
        Some(socket) => {
            live.push(socket);
            true
        }
        None => false,
    });
    live
}

fn portid_in_use(protocol: i32, portid: u32, skip: &NetlinkSocket) -> bool {
    if portid == 0 {
        return false;
    }
    live_sockets().into_iter().any(|socket| {
        socket.protocol == protocol
            && !core::ptr::addr_eq(Arc::as_ptr(&socket), skip as *const NetlinkSocket)
            && socket.state.lock().portid == portid
    })
}

fn allocate_portid(protocol: i32, preferred: u32, skip: &NetlinkSocket) -> u32 {
    if preferred != 0 && !portid_in_use(protocol, preferred, skip) {
        return preferred;
    }

    let mut candidate = preferred.max(1);
    while portid_in_use(protocol, candidate, skip) {
        candidate = candidate.saturating_add(1).max(1);
    }
    candidate
}

fn lookup_portid(protocol: i32, portid: u32, skip: &NetlinkSocket) -> Option<Arc<NetlinkSocket>> {
    live_sockets().into_iter().find(|socket| {
        socket.protocol == protocol
            && socket.state.lock().portid == portid
            && !core::ptr::addr_eq(Arc::as_ptr(socket), skip as *const NetlinkSocket)
    })
}

fn broadcast_message(protocol: i32, sender_portid: u32, group_mask: u32, payload: &[u8]) {
    if group_mask == 0 {
        return;
    }

    for socket in live_sockets() {
        if socket.protocol != protocol {
            continue;
        }
        let groups = socket.state.lock().groups;
        if (groups & group_mask) == 0 {
            continue;
        }
        let _ = socket.enqueue(NetlinkMessage {
            data: payload.to_vec(),
            sender_portid,
            sender_groups: group_mask,
        });
    }
}

fn read_sockopt_i32(value: &[u8]) -> SysResult<i32> {
    if value.len() < core::mem::size_of::<i32>() {
        return Err(SysErr::Inval);
    }
    Ok(i32::from_ne_bytes(
        value[..core::mem::size_of::<i32>()]
            .try_into()
            .map_err(|_| SysErr::Fault)?,
    ))
}

fn read_sockopt_u32(value: &[u8]) -> SysResult<u32> {
    if value.len() < core::mem::size_of::<u32>() {
        return Err(SysErr::Inval);
    }
    Ok(u32::from_ne_bytes(
        value[..core::mem::size_of::<u32>()]
            .try_into()
            .map_err(|_| SysErr::Fault)?,
    ))
}

fn normalize_socket_buffer(value: i32) -> usize {
    value
        .max(MIN_SOCKET_BUFFER as i32)
        .try_into()
        .unwrap_or(DEFAULT_SOCKET_BUFFER)
}

fn clamp_sockopt_i32(value: usize) -> i32 {
    value.min(i32::MAX as usize) as i32
}
