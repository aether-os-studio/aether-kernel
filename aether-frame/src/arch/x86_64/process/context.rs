use crate::arch::interrupt::TrapFrame;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct UserContext {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rbp: u64,
    pub rbx: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rax: u64,
    pub rip: u64,
    pub rsp: u64,
    pub rflags: u64,
    pub fs_base: u64,
    pub gs_base: u64,
}

impl UserContext {
    #[must_use]
    pub const fn new(entry: u64, user_stack_top: u64) -> Self {
        Self {
            r15: 0,
            r14: 0,
            r13: 0,
            r12: 0,
            r11: 0,
            r10: 0,
            r9: 0,
            r8: 0,
            rdi: 0,
            rsi: 0,
            rbp: 0,
            rbx: 0,
            rdx: 0,
            rcx: 0,
            rax: 0,
            rip: entry,
            rsp: user_stack_top,
            rflags: 1 << 9,
            fs_base: 0,
            gs_base: 0,
        }
    }

    pub(crate) const fn capture_from_trap(&mut self, frame: &TrapFrame) {
        self.r15 = frame.r15;
        self.r14 = frame.r14;
        self.r13 = frame.r13;
        self.r12 = frame.r12;
        self.r11 = frame.r11;
        self.r10 = frame.r10;
        self.r9 = frame.r9;
        self.r8 = frame.r8;
        self.rdi = frame.rdi;
        self.rsi = frame.rsi;
        self.rbp = frame.rbp;
        self.rbx = frame.rbx;
        self.rdx = frame.rdx;
        self.rcx = frame.rcx;
        self.rax = frame.rax;
        self.rip = frame.rip;
        self.rsp = frame.rsp;
        self.rflags = frame.rflags;
    }

    #[must_use]
    pub const fn fs_base(&self) -> u64 {
        self.fs_base
    }

    pub const fn set_fs_base(&mut self, value: u64) {
        self.fs_base = value;
    }

    #[must_use]
    pub const fn gs_base(&self) -> u64 {
        self.gs_base
    }

    pub const fn set_gs_base(&mut self, value: u64) {
        self.gs_base = value;
    }
}
