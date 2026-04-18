use aether_frame::process::UserContext;

pub trait ArchContext {
    fn syscall_number(&self) -> u64;
    fn syscall_args(&self) -> [u64; 6];
    fn set_return_value(&mut self, value: u64);
    fn instruction_pointer(&self) -> u64;
    #[allow(dead_code)]
    fn stack_pointer(&self) -> u64;
    fn set_stack_pointer(&mut self, value: u64);
    fn thread_pointer(&self) -> u64;
    fn set_thread_pointer(&mut self, value: u64);
    fn secondary_thread_pointer(&self) -> u64;
    fn set_secondary_thread_pointer(&mut self, value: u64);
}

impl ArchContext for UserContext {
    fn syscall_number(&self) -> u64 {
        self.general.rax
    }

    fn syscall_args(&self) -> [u64; 6] {
        [
            self.general.rdi,
            self.general.rsi,
            self.general.rdx,
            self.general.r10,
            self.general.r8,
            self.general.r9,
        ]
    }

    fn set_return_value(&mut self, value: u64) {
        self.general.rax = value;
    }

    fn instruction_pointer(&self) -> u64 {
        self.general.rip
    }

    fn stack_pointer(&self) -> u64 {
        self.general.rsp
    }

    fn set_stack_pointer(&mut self, value: u64) {
        self.general.rsp = value;
    }

    fn thread_pointer(&self) -> u64 {
        self.fs_base()
    }

    fn set_thread_pointer(&mut self, value: u64) {
        self.set_fs_base(value);
    }

    fn secondary_thread_pointer(&self) -> u64 {
        self.gs_base()
    }

    fn set_secondary_thread_pointer(&mut self, value: u64) {
        self.set_gs_base(value);
    }
}
