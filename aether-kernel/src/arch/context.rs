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
        self.rax
    }

    fn syscall_args(&self) -> [u64; 6] {
        [self.rdi, self.rsi, self.rdx, self.r10, self.r8, self.r9]
    }

    fn set_return_value(&mut self, value: u64) {
        self.rax = value;
    }

    fn instruction_pointer(&self) -> u64 {
        self.rip
    }

    fn stack_pointer(&self) -> u64 {
        self.rsp
    }

    fn set_stack_pointer(&mut self, value: u64) {
        self.rsp = value;
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
