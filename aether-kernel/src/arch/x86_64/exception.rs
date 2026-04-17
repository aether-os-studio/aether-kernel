use crate::arch::exception::{
    PageFaultAccessType, PageFaultDetails, UserExceptionClass, UserExceptionDetails,
};

const PAGE_FAULT_VECTOR: u8 = 14;

pub fn exception_signal(vector: u8) -> Option<u8> {
    match vector {
        0 => Some(crate::signal::SIGFPE),
        1 | 3 => Some(crate::signal::SIGTRAP),
        6 => Some(crate::signal::SIGILL),
        13 | PAGE_FAULT_VECTOR => Some(crate::signal::SIGSEGV),
        _ => None,
    }
}

pub fn classify_user_exception(details: UserExceptionDetails) -> UserExceptionClass {
    if details.vector == PAGE_FAULT_VECTOR {
        let error_code = details.error_code;
        let access = if (error_code & (1 << 4)) != 0 {
            PageFaultAccessType::Execute
        } else if (error_code & (1 << 1)) != 0 {
            PageFaultAccessType::Write
        } else {
            PageFaultAccessType::Read
        };

        return UserExceptionClass::PageFault(PageFaultDetails {
            address: details.fault_address,
            instruction_pointer: details.instruction_pointer,
            error_code,
            access,
            present: (error_code & 1) != 0,
            from_user: (error_code & (1 << 2)) != 0,
            reserved_bit: (error_code & (1 << 3)) != 0,
            instruction_fetch: (error_code & (1 << 4)) != 0,
        });
    }

    if let Some(signal) = exception_signal(details.vector) {
        UserExceptionClass::Signal { signal, details }
    } else {
        UserExceptionClass::Fatal(details)
    }
}
