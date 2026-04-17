use aether_drivers::DrmIoctlError;
use aether_frame::mm::MappingError;
use aether_process::BuildError;
use aether_vfs::FsError;

/// System error codes compatible with Linux x86_64.
///
/// The numeric values match the Linux kernel's error codes for x86_64 architecture.
/// When returned from a system call, these are negated to form the errno value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SysErr {
    /// Operation not permitted (EPERM = 1)
    Perm,
    /// No such file or directory (ENOENT = 2)
    NoEnt,
    /// No such process (ESRCH = 3)
    Srch,
    /// Interrupted system call (EINTR = 4)
    Intr,
    /// I/O error (EIO = 5)
    Io,
    /// No such device or address (ENXIO = 6)
    Nxio,
    /// Argument list too long (E2BIG = 7)
    TooBig,
    /// Exec format error (ENOEXEC = 8)
    NoExec,
    /// Bad file number (EBADF = 9)
    BadFd,
    /// No child processes (ECHILD = 10)
    Child,
    /// Try again (EAGAIN = 11)
    Again,
    /// Out of memory (ENOMEM = 12)
    NoMem,
    /// Permission denied (EACCES = 13)
    Access,
    /// Bad address (EFAULT = 14)
    Fault,
    /// Block device required (ENOTBLK = 15)
    NotBlk,
    /// Device or resource busy (EBUSY = 16)
    Busy,
    /// File exists (EEXIST = 17)
    Exists,
    /// Cross-device link (EXDEV = 18)
    XDev,
    /// No such device (ENODEV = 19)
    NoDev,
    /// Not a directory (ENOTDIR = 20)
    NotDir,
    /// Is a directory (EISDIR = 21)
    IsDir,
    /// Invalid argument (EINVAL = 22)
    Inval,
    /// File table overflow (ENFILE = 23)
    NFile,
    /// Too many open files (EMFILE = 24)
    MFile,
    /// Not a typewriter (ENOTTY = 25)
    NoTty,
    /// Text file busy (ETXTBSY = 26)
    TxtBsy,
    /// File too large (EFBIG = 27)
    FBig,
    /// No space left on device (ENOSPC = 28)
    NoSpc,
    /// Illegal seek (ESPIPE = 29)
    SPipe,
    /// Read-only file system (EROFS = 30)
    RoFs,
    /// Too many links (EMLINK = 31)
    MLink,
    /// Broken pipe (EPIPE = 32)
    Pipe,
    /// Math argument out of domain (EDOM = 33)
    Dom,
    /// Math result not representable (ERANGE = 34)
    Range,
    /// Resource deadlock would occur (EDEADLK = 35)
    DeadLk,
    /// File name too long (ENAMETOOLONG = 36)
    NameTooLong,
    /// No record locks available (ENOLCK = 37)
    NoLck,
    /// Function not implemented (ENOSYS = 38)
    NoSys,
    /// Directory not empty (ENOTEMPTY = 39)
    NotEmpty,
    /// Too many symbolic links encountered (ELOOP = 40)
    Loop,
    /// No message of desired type (ENOMSG = 42)
    NoMsg,
    /// Identifier removed (EIDRM = 43)
    IdRm,
    /// Channel number out of range (ECHRNG = 44)
    Chrng,
    /// Level 2 not synchronized (EL2NSYNC = 45)
    L2Nsyc,
    /// Level 3 halted (EL3HLT = 46)
    L3Hlt,
    /// Level 3 reset (EL3RST = 47)
    L3Rst,
    /// Link number out of range (ELNRNG = 48)
    LnRng,
    /// Protocol driver not attached (EUNATCH = 49)
    Unatch,
    /// No CSI structure available (ENOCSI = 50)
    NoCsi,
    /// Level 2 halted (EL2HLT = 51)
    L2Hlt,
    /// Invalid exchange (EBADE = 52)
    Bade,
    /// Invalid request descriptor (EBADR = 53)
    Badr,
    /// Exchange full (EXFULL = 54)
    XFull,
    /// No anode (ENOANO = 55)
    NoAno,
    /// Invalid request code (EBADRQC = 56)
    BadRqc,
    /// Invalid slot (EBADSLT = 57)
    BadSlt,
    /// Deadlock (EDEADLOCK = 58, same as EDEADLK on some architectures)
    DeadLock,
    /// Bad font file format (EBFONT = 59)
    Bfont,
    /// Device not a stream (ENOSTR = 60)
    NoStr,
    /// No data available (ENODATA = 61)
    NoData,
    /// Timer expired (ETIME = 62)
    Time,
    /// Out of streams resources (ENOSR = 63)
    NoSr,
    /// Machine is not on the network (ENONET = 64)
    NoNet,
    /// Package not installed (ENOPKG = 65)
    NoPkg,
    /// Object is remote (EREMOTE = 66)
    Remote,
    /// Link has been severed (ENOLINK = 67)
    NoLink,
    /// Advertise error (EADV = 68)
    Adv,
    /// Srmount error (ESRMNT = 69)
    SrMnt,
    /// Communication error on send (ECOMM = 70)
    Comm,
    /// Protocol error (EPROTO = 71)
    Proto,
    /// Multihop attempted (EMULTIHOP = 72)
    MultiHop,
    /// RFS specific error (EDOTDOT = 73)
    DotDot,
    /// Not a data message (EBADMSG = 74)
    BadMsg,
    /// Value too large for defined data type (EOVERFLOW = 75)
    Overflow,
    /// Name not unique on network (ENOTUNIQ = 76)
    NotUniq,
    /// File descriptor in bad state (EBADFD = 77)
    BadFdState,
    /// Remote address changed (EREMCHG = 78)
    RemChg,
    /// Can not access a needed shared library (ELIBACC = 79)
    LibAcc,
    /// Accessing a corrupted shared library (ELIBBAD = 80)
    LibBad,
    /// .lib section in a.out corrupted (ELIBSCN = 81)
    LibScn,
    /// Attempting to link in too many shared libraries (ELIBMAX = 82)
    LibMax,
    /// Cannot exec a shared library directly (ELIBEXEC = 83)
    LibExec,
    /// Illegal byte sequence (EILSEQ = 84)
    IlSeq,
    /// Interrupted system call should be restarted (ERESTART = 85)
    Restart,
    /// Streams pipe error (ESTRPIPE = 86)
    StrPipe,
    /// Too many users (EUSERS = 87)
    Users,
    /// Socket operation on non-socket (ENOTSOCK = 88)
    NotSock,
    /// Destination address required (EDESTADDRREQ = 89)
    DestAddrReq,
    /// Message too long (EMSGSIZE = 90)
    MsgSize,
    /// Protocol wrong type for socket (EPROTOTYPE = 91)
    ProtoType,
    /// Protocol not available (ENOPROTOOPT = 92)
    NoProtoOpt,
    /// Protocol not supported (EPROTONOSUPPORT = 93)
    ProtoNoSupport,
    /// Socket type not supported (ESOCKTNOSUPPORT = 94)
    SockTNoSupport,
    /// Operation not supported on transport endpoint (EOPNOTSUPP = 95)
    NotSup,
    /// Protocol family not supported (EPFNOSUPPORT = 96)
    PfNoSupport,
    /// Address family not supported by protocol (EAFNOSUPPORT = 97)
    AfNoSupport,
    /// Address already in use (EADDRINUSE = 98)
    AddrInUse,
    /// Cannot assign requested address (EADDRNOTAVAIL = 99)
    AddrNotAvail,
    /// Network is down (ENETDOWN = 100)
    NetDown,
    /// Network is unreachable (ENETUNREACH = 101)
    NetUnreach,
    /// Network dropped connection because of reset (ENETRESET = 102)
    NetReset,
    /// Software caused connection abort (ECONNABORTED = 103)
    ConnAborted,
    /// Connection reset by peer (ECONNRESET = 104)
    ConnReset,
    /// No buffer space available (ENOBUFS = 105)
    NoBufs,
    /// Transport endpoint is already connected (EISCONN = 106)
    IsConn,
    /// Transport endpoint is not connected (ENOTCONN = 107)
    NotConn,
    /// Cannot send after transport endpoint shutdown (ESHUTDOWN = 108)
    Shutdown,
    /// Too many references: cannot splice (ETOOMANYREFS = 109)
    TooManyRefs,
    /// Connection timed out (ETIMEDOUT = 110)
    TimedOut,
    /// Connection refused (ECONNREFUSED = 111)
    ConnRefused,
    /// Host is down (EHOSTDOWN = 112)
    HostDown,
    /// No route to host (EHOSTUNREACH = 113)
    HostUnreach,
    /// Operation already in progress (EALREADY = 114)
    Already,
    /// Operation now in progress (EINPROGRESS = 115)
    InProgress,
    /// Stale NFS file handle (ESTALE = 116)
    Stale,
    /// Structure needs cleaning (EUCLEAN = 117)
    Uclean,
    /// Not a XENIX named type file (ENOTNAM = 118)
    NotNam,
    /// No XENIX semaphores available (ENAVAIL = 119)
    Navail,
    /// Is a named type file (EISNAM = 120)
    IsNam,
    /// Remote I/O error (EREMOTEIO = 121)
    RemoteIo,
    /// Quota exceeded (EDQUOT = 122)
    DQuot,
    /// No medium found (ENOMEDIUM = 123)
    NoMedium,
    /// Wrong medium type (EMEDIUMTYPE = 124)
    MediumType,
    /// Operation canceled (ECANCELED = 125)
    Canceled,
    /// Required key not available (ENOKEY = 126)
    NoKey,
    /// Key has expired (EKEYEXPIRED = 127)
    KeyExpired,
    /// Key has been revoked (EKEYREVOKED = 128)
    KeyRevoked,
    /// Key was rejected by service (EKEYREJECTED = 129)
    KeyRejected,
    /// Owner died (EOWNERDEAD = 130)
    OwnerDead,
    /// State not recoverable (ENOTRECOVERABLE = 131)
    NotRecoverable,
    /// Operation not possible due to RF-kill (ERFKILL = 132)
    RfKill,
    /// Memory page has hardware error (EHWPOISON = 133)
    HwPoison,
}

pub type SysResult<T> = Result<T, SysErr>;

impl SysErr {
    /// Returns the Linux error code number for this error.
    ///
    /// These values match the Linux x86_64 architecture error codes.
    pub const fn code(self) -> i64 {
        match self {
            Self::Perm => 1,
            Self::NoEnt => 2,
            Self::Srch => 3,
            Self::Intr => 4,
            Self::Io => 5,
            Self::Nxio => 6,
            Self::TooBig => 7,
            Self::NoExec => 8,
            Self::BadFd => 9,
            Self::Child => 10,
            Self::Again => 11,
            Self::NoMem => 12,
            Self::Access => 13,
            Self::Fault => 14,
            Self::NotBlk => 15,
            Self::Busy => 16,
            Self::Exists => 17,
            Self::XDev => 18,
            Self::NoDev => 19,
            Self::NotDir => 20,
            Self::IsDir => 21,
            Self::Inval => 22,
            Self::NFile => 23,
            Self::MFile => 24,
            Self::NoTty => 25,
            Self::TxtBsy => 26,
            Self::FBig => 27,
            Self::NoSpc => 28,
            Self::SPipe => 29,
            Self::RoFs => 30,
            Self::MLink => 31,
            Self::Pipe => 32,
            Self::Dom => 33,
            Self::Range => 34,
            Self::DeadLk => 35,
            Self::NameTooLong => 36,
            Self::NoLck => 37,
            Self::NoSys => 38,
            Self::NotEmpty => 39,
            Self::Loop => 40,
            // 41 is EWOULDBLOCK, same as EAGAIN
            Self::NoMsg => 42,
            Self::IdRm => 43,
            Self::Chrng => 44,
            Self::L2Nsyc => 45,
            Self::L3Hlt => 46,
            Self::L3Rst => 47,
            Self::LnRng => 48,
            Self::Unatch => 49,
            Self::NoCsi => 50,
            Self::L2Hlt => 51,
            Self::Bade => 52,
            Self::Badr => 53,
            Self::XFull => 54,
            Self::NoAno => 55,
            Self::BadRqc => 56,
            Self::BadSlt => 57,
            Self::DeadLock => 58,
            Self::Bfont => 59,
            Self::NoStr => 60,
            Self::NoData => 61,
            Self::Time => 62,
            Self::NoSr => 63,
            Self::NoNet => 64,
            Self::NoPkg => 65,
            Self::Remote => 66,
            Self::NoLink => 67,
            Self::Adv => 68,
            Self::SrMnt => 69,
            Self::Comm => 70,
            Self::Proto => 71,
            Self::MultiHop => 72,
            Self::DotDot => 73,
            Self::BadMsg => 74,
            Self::Overflow => 75,
            Self::NotUniq => 76,
            Self::BadFdState => 77,
            Self::RemChg => 78,
            Self::LibAcc => 79,
            Self::LibBad => 80,
            Self::LibScn => 81,
            Self::LibMax => 82,
            Self::LibExec => 83,
            Self::IlSeq => 84,
            Self::Restart => 85,
            Self::StrPipe => 86,
            Self::Users => 87,
            Self::NotSock => 88,
            Self::DestAddrReq => 89,
            Self::MsgSize => 90,
            Self::ProtoType => 91,
            Self::NoProtoOpt => 92,
            Self::ProtoNoSupport => 93,
            Self::SockTNoSupport => 94,
            Self::NotSup => 95,
            Self::PfNoSupport => 96,
            Self::AfNoSupport => 97,
            Self::AddrInUse => 98,
            Self::AddrNotAvail => 99,
            Self::NetDown => 100,
            Self::NetUnreach => 101,
            Self::NetReset => 102,
            Self::ConnAborted => 103,
            Self::ConnReset => 104,
            Self::NoBufs => 105,
            Self::IsConn => 106,
            Self::NotConn => 107,
            Self::Shutdown => 108,
            Self::TooManyRefs => 109,
            Self::TimedOut => 110,
            Self::ConnRefused => 111,
            Self::HostDown => 112,
            Self::HostUnreach => 113,
            Self::Already => 114,
            Self::InProgress => 115,
            Self::Stale => 116,
            Self::Uclean => 117,
            Self::NotNam => 118,
            Self::Navail => 119,
            Self::IsNam => 120,
            Self::RemoteIo => 121,
            Self::DQuot => 122,
            Self::NoMedium => 123,
            Self::MediumType => 124,
            Self::Canceled => 125,
            Self::NoKey => 126,
            Self::KeyExpired => 127,
            Self::KeyRevoked => 128,
            Self::KeyRejected => 129,
            Self::OwnerDead => 130,
            Self::NotRecoverable => 131,
            Self::RfKill => 132,
            Self::HwPoison => 133,
        }
    }

    /// Returns the negative error code suitable for returning from system calls.
    ///
    /// In Linux, system calls return negative error codes on failure.
    /// This is what gets stored in errno after the syscall returns.
    pub const fn errno(self) -> i64 {
        -(self.code())
    }

    /// Creates a SysErr from a Linux error number.
    ///
    /// Returns None if the error number doesn't correspond to a known error.
    pub const fn from_code(code: i64) -> Option<Self> {
        Some(match code {
            1 => Self::Perm,
            2 => Self::NoEnt,
            3 => Self::Srch,
            4 => Self::Intr,
            5 => Self::Io,
            6 => Self::Nxio,
            7 => Self::TooBig,
            8 => Self::NoExec,
            9 => Self::BadFd,
            10 => Self::Child,
            11 => Self::Again,
            12 => Self::NoMem,
            13 => Self::Access,
            14 => Self::Fault,
            15 => Self::NotBlk,
            16 => Self::Busy,
            17 => Self::Exists,
            18 => Self::XDev,
            19 => Self::NoDev,
            20 => Self::NotDir,
            21 => Self::IsDir,
            22 => Self::Inval,
            23 => Self::NFile,
            24 => Self::MFile,
            25 => Self::NoTty,
            26 => Self::TxtBsy,
            27 => Self::FBig,
            28 => Self::NoSpc,
            29 => Self::SPipe,
            30 => Self::RoFs,
            31 => Self::MLink,
            32 => Self::Pipe,
            33 => Self::Dom,
            34 => Self::Range,
            35 => Self::DeadLk,
            36 => Self::NameTooLong,
            37 => Self::NoLck,
            38 => Self::NoSys,
            39 => Self::NotEmpty,
            40 => Self::Loop,
            42 => Self::NoMsg,
            43 => Self::IdRm,
            44 => Self::Chrng,
            45 => Self::L2Nsyc,
            46 => Self::L3Hlt,
            47 => Self::L3Rst,
            48 => Self::LnRng,
            49 => Self::Unatch,
            50 => Self::NoCsi,
            51 => Self::L2Hlt,
            52 => Self::Bade,
            53 => Self::Badr,
            54 => Self::XFull,
            55 => Self::NoAno,
            56 => Self::BadRqc,
            57 => Self::BadSlt,
            58 => Self::DeadLock,
            59 => Self::Bfont,
            60 => Self::NoStr,
            61 => Self::NoData,
            62 => Self::Time,
            63 => Self::NoSr,
            64 => Self::NoNet,
            65 => Self::NoPkg,
            66 => Self::Remote,
            67 => Self::NoLink,
            68 => Self::Adv,
            69 => Self::SrMnt,
            70 => Self::Comm,
            71 => Self::Proto,
            72 => Self::MultiHop,
            73 => Self::DotDot,
            74 => Self::BadMsg,
            75 => Self::Overflow,
            76 => Self::NotUniq,
            77 => Self::BadFdState,
            78 => Self::RemChg,
            79 => Self::LibAcc,
            80 => Self::LibBad,
            81 => Self::LibScn,
            82 => Self::LibMax,
            83 => Self::LibExec,
            84 => Self::IlSeq,
            85 => Self::Restart,
            86 => Self::StrPipe,
            87 => Self::Users,
            88 => Self::NotSock,
            89 => Self::DestAddrReq,
            90 => Self::MsgSize,
            91 => Self::ProtoType,
            92 => Self::NoProtoOpt,
            93 => Self::ProtoNoSupport,
            94 => Self::SockTNoSupport,
            95 => Self::NotSup,
            96 => Self::PfNoSupport,
            97 => Self::AfNoSupport,
            98 => Self::AddrInUse,
            99 => Self::AddrNotAvail,
            100 => Self::NetDown,
            101 => Self::NetUnreach,
            102 => Self::NetReset,
            103 => Self::ConnAborted,
            104 => Self::ConnReset,
            105 => Self::NoBufs,
            106 => Self::IsConn,
            107 => Self::NotConn,
            108 => Self::Shutdown,
            109 => Self::TooManyRefs,
            110 => Self::TimedOut,
            111 => Self::ConnRefused,
            112 => Self::HostDown,
            113 => Self::HostUnreach,
            114 => Self::Already,
            115 => Self::InProgress,
            116 => Self::Stale,
            117 => Self::Uclean,
            118 => Self::NotNam,
            119 => Self::Navail,
            120 => Self::IsNam,
            121 => Self::RemoteIo,
            122 => Self::DQuot,
            123 => Self::NoMedium,
            124 => Self::MediumType,
            125 => Self::Canceled,
            126 => Self::NoKey,
            127 => Self::KeyExpired,
            128 => Self::KeyRevoked,
            129 => Self::KeyRejected,
            130 => Self::OwnerDead,
            131 => Self::NotRecoverable,
            132 => Self::RfKill,
            133 => Self::HwPoison,
            _ => return None,
        })
    }
}

impl From<FsError> for SysErr {
    fn from(value: FsError) -> Self {
        match value {
            FsError::NotFound => Self::NoEnt,
            FsError::NotDirectory => Self::NotDir,
            FsError::NotFile => Self::Inval,
            FsError::AlreadyExists => Self::Exists,
            FsError::Unsupported => Self::NotSup,
            FsError::InvalidInput | FsError::RootNotMounted => Self::Inval,
            FsError::WouldBlock => Self::Again,
            FsError::BrokenPipe => Self::Pipe,
        }
    }
}

impl From<DrmIoctlError> for SysErr {
    fn from(value: DrmIoctlError) -> Self {
        match value {
            DrmIoctlError::Invalid => Self::Inval,
            DrmIoctlError::NotFound => Self::NoEnt,
            DrmIoctlError::Busy => Self::Busy,
            DrmIoctlError::Permission => Self::Perm,
            DrmIoctlError::NotSupported => Self::NotSup,
            DrmIoctlError::NoMemory => Self::NoMem,
        }
    }
}

impl From<BuildError> for SysErr {
    fn from(value: BuildError) -> Self {
        match value {
            BuildError::Frame(_) => Self::NoMem,
            BuildError::Map(MappingError::OutOfMemory | MappingError::Frame(_)) => Self::NoMem,
            BuildError::Map(MappingError::NotMapped) => Self::Fault,
            BuildError::AddressOverflow
            | BuildError::StackOverflow
            | BuildError::InvalidElf
            | BuildError::UnsupportedElf
            | BuildError::Image(_)
            | BuildError::EmptyProgram
            | BuildError::Map(_) => Self::Inval,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_errno_roundtrip() {
        // Test that errno() returns negative of code()
        assert_eq!(SysErr::NoEnt.errno(), -2);
        assert_eq!(SysErr::BadFd.errno(), -9);
        assert_eq!(SysErr::Intr.errno(), -4);
    }

    #[test]
    fn test_from_code() {
        assert_eq!(SysErr::from_code(2), Some(SysErr::NoEnt));
        assert_eq!(SysErr::from_code(9), Some(SysErr::BadFd));
        assert_eq!(SysErr::from_code(1000), None);
    }
}
