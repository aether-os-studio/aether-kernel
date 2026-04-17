#[derive(Debug, Clone, Copy)]
pub struct UserAddressSpaceLayout {
    pub region_base: u64,
    pub region_align: u64,
    pub pie_base: u64,
    pub interpreter_base: u64,
    pub stack_end: u64,
    pub mmap_top: u64,
    pub brk_reserve: u64,
    pub default_elf_stack_pages: usize,
}

impl UserAddressSpaceLayout {
    #[cfg(target_arch = "x86_64")]
    pub const fn current() -> Self {
        const USER_VA_ALIGN: u64 = 0x20_0000;
        const INTERPRETER_BASE: u64 = 0x0000_6ff0_0000_0000;

        Self {
            region_base: 0x0000_0000_4000_0000,
            region_align: USER_VA_ALIGN,
            pie_base: 0x0000_6000_0000_0000,
            interpreter_base: INTERPRETER_BASE,
            stack_end: 0x0000_7000_0000_0000,
            mmap_top: INTERPRETER_BASE - USER_VA_ALIGN,
            brk_reserve: 16 * 1024 * 1024,
            default_elf_stack_pages: 64,
        }
    }
}
