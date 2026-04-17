#![allow(dead_code)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;

use aether_frame::libs::spin::SpinLock;
use aether_vfs::{
    FileNode, FileOperations, FsError, FsResult, IoctlResponse, MmapRequest, MmapResponse, NodeRef,
    OpenFlags, PollEvents, SharedWaitListener,
};

use crate::errno::{SysErr, SysResult};
use crate::fs::FileDescriptor;

mod netlink;
mod unix;

pub const SOL_SOCKET: i32 = 1;
pub const SO_TYPE: i32 = 3;
pub const SO_ERROR: i32 = 4;
pub const SO_SNDBUF: i32 = 7;
pub const SO_RCVBUF: i32 = 8;
pub const SO_PASSCRED: i32 = 16;
pub const SO_PEERCRED: i32 = 17;
pub const SO_ACCEPTCONN: i32 = 30;
pub const SO_SNDBUFFORCE: i32 = 32;
pub const SO_RCVBUFFORCE: i32 = 33;
pub const SO_PROTOCOL: i32 = 38;
pub const SO_DOMAIN: i32 = 39;
pub const SCM_RIGHTS: i32 = 1;
pub const SCM_CREDENTIALS: i32 = 2;

const SOCK_TYPE_MASK: u64 = 0xf;
const SOCK_NONBLOCK: u64 = 0o0004000;
const SOCK_CLOEXEC: u64 = 0o2000000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SocketType {
    kind: u32,
    nonblock: bool,
    cloexec: bool,
}

impl SocketType {
    pub fn parse(raw: u64) -> SysResult<Self> {
        let extra = raw & !(SOCK_TYPE_MASK | SOCK_NONBLOCK | SOCK_CLOEXEC);
        if extra != 0 {
            return Err(SysErr::Inval);
        }

        let kind = (raw & SOCK_TYPE_MASK) as u32;
        match kind {
            1 | 2 | 3 | 4 | 5 | 6 | 10 => {}
            _ => return Err(SysErr::Inval),
        }

        Ok(Self {
            kind,
            nonblock: (raw & SOCK_NONBLOCK) != 0,
            cloexec: (raw & SOCK_CLOEXEC) != 0,
        })
    }

    pub const fn kind(self) -> u32 {
        self.kind
    }

    pub const fn nonblock(self) -> bool {
        self.nonblock
    }

    pub const fn cloexec(self) -> bool {
        self.cloexec
    }

    pub fn open_flags(self) -> OpenFlags {
        let mut bits = OpenFlags::READ | OpenFlags::WRITE;
        if self.nonblock {
            bits |= OpenFlags::NONBLOCK;
        }
        OpenFlags::from_bits(bits)
    }
}

#[derive(Clone)]
pub struct SocketMessage {
    pub name: Option<Vec<u8>>,
    pub data: Vec<u8>,
    pub control: Vec<u8>,
    pub rights: Vec<FileDescriptor>,
    pub sender: SocketCredentials,
    pub explicit_credentials: Option<SocketCredentials>,
    pub msg_flags: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SocketCredentials {
    pub pid: u32,
    pub uid: u32,
    pub gid: u32,
}

impl SocketCredentials {
    pub const fn new(pid: u32, uid: u32, gid: u32) -> Self {
        Self { pid, uid, gid }
    }
}

pub struct AcceptedSocket {
    pub socket: Arc<dyn KernelSocket>,
    pub address: Option<Vec<u8>>,
}

pub struct SocketReceive {
    pub address: Option<Vec<u8>>,
    pub control: Vec<u8>,
    pub rights: Vec<FileDescriptor>,
    pub credentials: Option<SocketCredentials>,
    pub msg_flags: u32,
    pub bytes_read: usize,
}

pub trait KernelSocket: Any + Send + Sync {
    fn as_any(&self) -> &dyn Any;

    fn domain(&self) -> i32;

    fn socket_type(&self) -> u32;

    fn protocol(&self) -> i32 {
        0
    }

    fn read(&self, _buffer: &mut [u8]) -> FsResult<usize> {
        Err(FsError::Unsupported)
    }

    fn write(&self, _buffer: &[u8]) -> FsResult<usize> {
        Err(FsError::Unsupported)
    }

    fn recv_from(&self, buffer: &mut [u8], _flags: u64) -> SysResult<SocketReceive> {
        let bytes_read = self.read(buffer).map_err(SysErr::from)?;
        Ok(SocketReceive {
            address: None,
            control: Vec::new(),
            rights: Vec::new(),
            credentials: None,
            msg_flags: 0,
            bytes_read,
        })
    }

    fn recv_msg(&self, buffer: &mut [u8], flags: u64) -> SysResult<SocketReceive> {
        self.recv_from(buffer, flags)
    }

    fn shutdown(&self, _how: i32) -> SysResult<()> {
        Err(SysErr::NoSys)
    }

    fn sock_name(&self) -> SysResult<Vec<u8>> {
        Err(SysErr::NoSys)
    }

    fn peer_name(&self) -> SysResult<Vec<u8>> {
        Err(SysErr::NoSys)
    }

    fn connect(&self, _address: &[u8]) -> SysResult<()> {
        Err(SysErr::NoSys)
    }

    fn bind(&self, _address: &[u8]) -> SysResult<()> {
        Err(SysErr::NoSys)
    }

    fn listen(&self, _backlog: i32) -> SysResult<()> {
        Err(SysErr::NoSys)
    }

    fn accept(&self) -> SysResult<AcceptedSocket> {
        Err(SysErr::Again)
    }

    fn send_to(&self, _buffer: &[u8], _address: Option<&[u8]>, _flags: u64) -> SysResult<usize> {
        Err(SysErr::NoSys)
    }

    fn send_msg(&self, message: &SocketMessage, flags: u64) -> SysResult<usize> {
        self.send_to(message.data.as_slice(), message.name.as_deref(), flags)
    }

    fn setsockopt(&self, _level: i32, _optname: i32, _value: &[u8]) -> SysResult<()> {
        Err(SysErr::NoProtoOpt)
    }

    fn getsockopt(&self, level: i32, optname: i32) -> SysResult<Vec<u8>> {
        if level == SOL_SOCKET {
            return self.getsockopt_sol_socket(optname);
        }
        Err(SysErr::NoProtoOpt)
    }

    fn getsockopt_sol_socket(&self, optname: i32) -> SysResult<Vec<u8>> {
        match optname {
            SO_TYPE => Ok(encode_sockopt_i32(self.socket_type() as i32)),
            SO_ERROR => Ok(encode_sockopt_i32(0)),
            SO_ACCEPTCONN => Ok(encode_sockopt_i32(self.is_listening() as i32)),
            SO_PROTOCOL => Ok(encode_sockopt_i32(self.protocol())),
            SO_DOMAIN => Ok(encode_sockopt_i32(self.domain())),
            _ => Err(SysErr::NoProtoOpt),
        }
    }

    fn is_listening(&self) -> bool {
        false
    }

    fn poll(&self, _events: PollEvents) -> FsResult<PollEvents> {
        Ok(PollEvents::empty())
    }

    fn register_waiter(
        &self,
        _events: PollEvents,
        _listener: SharedWaitListener,
    ) -> FsResult<Option<u64>> {
        Ok(None)
    }

    fn unregister_waiter(&self, _waiter_id: u64) -> FsResult<()> {
        Ok(())
    }
}

pub trait SocketDomain: Send + Sync {
    fn domain(&self) -> i32;
    fn create(
        &self,
        socket_type: SocketType,
        protocol: i32,
        owner: SocketCredentials,
    ) -> SysResult<Arc<dyn KernelSocket>>;

    fn create_pair(
        &self,
        _socket_type: SocketType,
        _protocol: i32,
        _owner: SocketCredentials,
    ) -> SysResult<(Arc<dyn KernelSocket>, Arc<dyn KernelSocket>)> {
        Err(SysErr::NoSys)
    }
}

pub struct SocketFile {
    socket: Arc<dyn KernelSocket>,
}

impl SocketFile {
    pub fn new(socket: Arc<dyn KernelSocket>) -> Self {
        Self { socket }
    }

    pub fn socket(&self) -> Arc<dyn KernelSocket> {
        self.socket.clone()
    }
}

impl FileOperations for SocketFile {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn read(&self, _offset: usize, buffer: &mut [u8]) -> FsResult<usize> {
        self.socket.read(buffer)
    }

    fn write(&self, _offset: usize, buffer: &[u8]) -> FsResult<usize> {
        self.socket.write(buffer)
    }

    fn ioctl(&self, _command: u64, _argument: u64) -> FsResult<IoctlResponse> {
        Err(FsError::Unsupported)
    }

    fn poll(&self, events: PollEvents) -> FsResult<PollEvents> {
        self.socket.poll(events)
    }

    fn register_waiter(
        &self,
        events: PollEvents,
        listener: SharedWaitListener,
    ) -> FsResult<Option<u64>> {
        self.socket.register_waiter(events, listener)
    }

    fn unregister_waiter(&self, waiter_id: u64) -> FsResult<()> {
        self.socket.unregister_waiter(waiter_id)
    }

    fn mmap(&self, _request: MmapRequest) -> FsResult<MmapResponse> {
        Ok(MmapResponse::buffered())
    }
}

pub struct CreatedSocket {
    pub node: NodeRef,
    pub flags: OpenFlags,
    pub cloexec: bool,
}

pub struct CreatedSocketPair {
    pub first: NodeRef,
    pub second: NodeRef,
    pub flags: OpenFlags,
    pub cloexec: bool,
}

pub struct SocketDomainRegistry {
    entries: SpinLock<BTreeMap<i32, &'static dyn SocketDomain>>,
}

impl SocketDomainRegistry {
    pub const fn new() -> Self {
        Self {
            entries: SpinLock::new(BTreeMap::new()),
        }
    }

    pub fn register(&self, domain: &'static dyn SocketDomain) {
        self.entries.lock().insert(domain.domain(), domain);
    }

    pub fn get(&self, domain: i32) -> Option<&'static dyn SocketDomain> {
        self.entries.lock().get(&domain).copied()
    }
}

static REGISTRY: SocketDomainRegistry = SocketDomainRegistry::new();

pub fn registry() -> &'static SocketDomainRegistry {
    &REGISTRY
}

pub fn register_socket_domain(domain: &'static dyn SocketDomain) {
    REGISTRY.register(domain);
}

#[macro_export]
macro_rules! register_socket_domains {
    ($registry:expr, [$($handler:expr),* $(,)?]) => {{
        $( $registry.register($handler); )*
    }};
}

pub fn init() {
    unix::init();
    netlink::init();
}

pub use self::netlink::{
    current_kobject_uevent_seqnum, publish_kobject_uevent, reserve_kobject_uevent_seqnum,
};
pub use self::unix::pathname_from_raw as unix_pathname_from_raw;

pub fn create_socket(
    domain: i32,
    raw_type: u64,
    protocol: i32,
    owner: SocketCredentials,
) -> SysResult<CreatedSocket> {
    let socket_type = SocketType::parse(raw_type)?;
    let factory = REGISTRY.get(domain).ok_or(SysErr::AfNoSupport)?;
    let socket = factory.create(socket_type, protocol, owner)?;
    let node: NodeRef = FileNode::new_socket("socket", 0o140777, Arc::new(SocketFile::new(socket)));
    Ok(CreatedSocket {
        node,
        flags: socket_type.open_flags(),
        cloexec: socket_type.cloexec(),
    })
}

pub fn create_socket_pair(
    domain: i32,
    raw_type: u64,
    protocol: i32,
    owner: SocketCredentials,
) -> SysResult<CreatedSocketPair> {
    let socket_type = SocketType::parse(raw_type)?;
    let factory = REGISTRY.get(domain).ok_or(SysErr::AfNoSupport)?;
    let (first_socket, second_socket) = factory.create_pair(socket_type, protocol, owner)?;
    let first: NodeRef =
        FileNode::new_socket("socket", 0o140777, Arc::new(SocketFile::new(first_socket)));
    let second: NodeRef =
        FileNode::new_socket("socket", 0o140777, Arc::new(SocketFile::new(second_socket)));
    Ok(CreatedSocketPair {
        first,
        second,
        flags: socket_type.open_flags(),
        cloexec: socket_type.cloexec(),
    })
}

pub fn encode_sockopt_i32(value: i32) -> Vec<u8> {
    value.to_ne_bytes().to_vec()
}
