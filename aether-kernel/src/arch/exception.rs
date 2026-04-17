#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageFaultAccessType {
    Read,
    Write,
    Execute,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserExceptionDetails {
    pub vector: u8,
    pub error_code: u64,
    pub fault_address: u64,
    pub instruction_pointer: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageFaultDetails {
    pub address: u64,
    pub instruction_pointer: u64,
    pub error_code: u64,
    pub access: PageFaultAccessType,
    pub present: bool,
    pub from_user: bool,
    pub reserved_bit: bool,
    pub instruction_fetch: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserExceptionClass {
    PageFault(PageFaultDetails),
    Signal {
        signal: u8,
        details: UserExceptionDetails,
    },
    Fatal(UserExceptionDetails),
}

pub fn classify_user_exception(details: UserExceptionDetails) -> UserExceptionClass {
    #[cfg(target_arch = "x86_64")]
    {
        return crate::arch::x86_64::exception::classify_user_exception(details);
    }

    #[allow(unreachable_code)]
    UserExceptionClass::Fatal(details)
}
