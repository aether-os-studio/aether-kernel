use crate::arch::interrupt::TrapFrame;

pub const SYSCALL_TRAP_VECTOR: u8 = 0x80;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrapKind {
    Exception,
    Interrupt,
    Syscall,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivilegeLevel {
    Kernel,
    User,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Trap {
    vector: u8,
    error_code: u64,
    kind: TrapKind,
    privilege: PrivilegeLevel,
}

impl Trap {
    #[must_use]
    pub const fn from_frame(frame: &TrapFrame) -> Self {
        let vector = frame.vector();
        let kind = if frame.is_syscall() || vector == SYSCALL_TRAP_VECTOR {
            TrapKind::Syscall
        } else if vector < 32 {
            TrapKind::Exception
        } else {
            TrapKind::Interrupt
        };

        Self {
            vector,
            error_code: frame.error_code(),
            kind,
            privilege: if frame.from_user() {
                PrivilegeLevel::User
            } else {
                PrivilegeLevel::Kernel
            },
        }
    }

    #[must_use]
    pub const fn vector(self) -> u8 {
        self.vector
    }

    #[must_use]
    pub const fn error_code(self) -> u64 {
        self.error_code
    }

    #[must_use]
    pub const fn kind(self) -> TrapKind {
        self.kind
    }

    #[must_use]
    pub const fn privilege(self) -> PrivilegeLevel {
        self.privilege
    }
}
