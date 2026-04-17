use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::mem::size_of;
use core::sync::atomic::{AtomicU64, Ordering};

use aether_frame::boot::phys_to_virt;
use aether_frame::interrupt::timer;
use aether_frame::libs::spin::SpinLock;
use aether_frame::mm::{
    AddressSpace, ArchitecturePageTable, FrameAllocator, MapFlags, MapSize, MappingError,
    PAGE_SIZE, PhysAddr, PhysFrame, VirtAddr, frame_allocator, new_user_root,
};
use aether_frame::process::{Process, ProcessBuilder};

use crate::image::{ElfImage, ElfSegmentFlags, ElfSegmentType, ImageError, ProgramImageSource};
use crate::layout::UserAddressSpaceLayout;
const DEFAULT_EXECFN: &str = "<aether-process>";
const ELF64_PHDR_SIZE: u64 = 56;
const MAP_FIXED: u64 = 0x10;
const MAP_ANONYMOUS: u64 = 0x20;

const AT_NULL: u64 = 0;
const AT_PHDR: u64 = 3;
const AT_PHENT: u64 = 4;
const AT_PHNUM: u64 = 5;
const AT_PAGESZ: u64 = 6;
const AT_BASE: u64 = 7;
const AT_FLAGS: u64 = 8;
const AT_ENTRY: u64 = 9;
const AT_UID: u64 = 11;
const AT_EUID: u64 = 12;
const AT_GID: u64 = 13;
const AT_EGID: u64 = 14;
const AT_PLATFORM: u64 = 15;
const AT_HWCAP: u64 = 16;
const AT_CLKTCK: u64 = 17;
const AT_SECURE: u64 = 23;
const AT_BASE_PLATFORM: u64 = 24;
const AT_RANDOM: u64 = 25;
const AT_HWCAP2: u64 = 26;
const AT_EXECFN: u64 = 31;
const AT_SYSINFO_EHDR: u64 = 33;
const AT_MINSIGSTKSZ: u64 = 51;

static AUX_RANDOM_SEED: AtomicU64 = AtomicU64::new(0x4155_5856_5345_4544);

#[derive(Debug)]
pub enum BuildError {
    EmptyProgram,
    AddressOverflow,
    StackOverflow,
    InvalidElf,
    UnsupportedElf,
    Image(ImageError),
    Map(MappingError),
    Frame(aether_frame::mm::FrameAllocError),
}

impl From<MappingError> for BuildError {
    fn from(value: MappingError) -> Self {
        Self::Map(value)
    }
}

impl From<aether_frame::mm::FrameAllocError> for BuildError {
    fn from(value: aether_frame::mm::FrameAllocError) -> Self {
        Self::Frame(value)
    }
}

impl From<ImageError> for BuildError {
    fn from(value: ImageError) -> Self {
        match value {
            ImageError::InvalidElf => Self::InvalidElf,
            ImageError::UnsupportedElf => Self::UnsupportedElf,
            other => Self::Image(other),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuxEntry {
    pub key: u64,
    pub value: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct LinuxAuxVector {
    pub phdr: u64,
    pub phent: u64,
    pub phnum: u64,
    pub base: u64,
    pub entry: u64,
    pub uid: u64,
    pub euid: u64,
    pub gid: u64,
    pub egid: u64,
    pub clock_tick: u64,
    pub page_size: u64,
    pub flags: u64,
    pub secure: u64,
    pub hwcap: u64,
    pub hwcap2: u64,
    pub sysinfo_ehdr: u64,
    pub minsigstksz: u64,
}

impl LinuxAuxVector {
    pub const fn new() -> Self {
        Self {
            phdr: 0,
            phent: ELF64_PHDR_SIZE,
            phnum: 0,
            base: 0,
            entry: 0,
            uid: 0,
            euid: 0,
            gid: 0,
            egid: 0,
            clock_tick: 100,
            page_size: PAGE_SIZE,
            flags: 0,
            secure: 0,
            hwcap: 0,
            hwcap2: 0,
            sysinfo_ehdr: 0,
            minsigstksz: 2048,
        }
    }

    fn with_entry(mut self, entry: u64) -> Self {
        if self.entry == 0 {
            self.entry = entry;
        }
        self
    }
}

impl Default for LinuxAuxVector {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct LinuxStackInfo {
    pub stack_pointer: u64,
    pub arg_start: u64,
    pub arg_end: u64,
    pub env_start: u64,
    pub env_end: u64,
    pub execfn_ptr: u64,
    pub random_ptr: u64,
}

pub struct BuiltProcess {
    pub process: Process,
    pub address_space: UserAddressSpace,
    pub stack: LinuxStackInfo,
}

impl BuiltProcess {
    pub fn fork_cow(&self) -> Result<Self, BuildError> {
        let address_space = self.address_space.fork_cow()?;
        let process = self.process.fork_with_root(address_space.root());

        Ok(Self {
            process,
            address_space,
            stack: self.stack,
        })
    }

    pub fn fork_copy(&self) -> Result<Self, BuildError> {
        let address_space = self.address_space.fork_copy()?;
        let process = self.process.fork_with_root(address_space.root());

        Ok(Self {
            process,
            address_space,
            stack: self.stack,
        })
    }

    pub fn fork_shared_vm(&self) -> Result<Self, BuildError> {
        let address_space = self.address_space.share();
        let process = self.process.fork_with_root(address_space.root());

        Ok(Self {
            process,
            address_space,
            stack: self.stack,
        })
    }
}

pub struct UserProgramBuilder<'a, S: ProgramImageSource + ?Sized> {
    code: &'a S,
    stack_pages: usize,
    argv: &'a [&'a str],
    envp: &'a [&'a str],
    execfn: Option<&'a str>,
    auxv: LinuxAuxVector,
}

pub struct ElfProgramBuilder<'a, S: ProgramImageSource + ?Sized> {
    executable: &'a S,
    interpreter: Option<&'a S>,
    argv: &'a [&'a str],
    envp: &'a [&'a str],
    execfn: Option<&'a str>,
    stack_pages: usize,
}

impl<'a, S: ProgramImageSource + ?Sized> UserProgramBuilder<'a, S> {
    pub const fn new(code: &'a S) -> Self {
        Self {
            code,
            stack_pages: 4,
            argv: &[],
            envp: &[],
            execfn: None,
            auxv: LinuxAuxVector::new(),
        }
    }

    pub fn stack_pages(mut self, stack_pages: usize) -> Self {
        self.stack_pages = stack_pages.max(1);
        self
    }

    pub fn argv(mut self, argv: &'a [&'a str]) -> Self {
        self.argv = argv;
        self
    }

    pub fn envp(mut self, envp: &'a [&'a str]) -> Self {
        self.envp = envp;
        self
    }

    pub fn execfn(mut self, execfn: &'a str) -> Self {
        self.execfn = Some(execfn);
        self
    }

    pub fn auxv(mut self, auxv: LinuxAuxVector) -> Self {
        self.auxv = auxv;
        self
    }

    pub fn build(self) -> Result<BuiltProcess, BuildError> {
        if self.code.len() == 0 {
            return Err(BuildError::EmptyProgram);
        }

        let code_pages = pages_for(self.code.len());
        let stack_pages = self.stack_pages.max(1);
        let total_pages = code_pages + stack_pages;
        let mut address_space = UserAddressSpace::new()?;
        let region_base = address_space.allocate_region(total_pages as u64 * PAGE_SIZE)?;

        let code_base = region_base;
        let stack_base = region_base
            .checked_add(code_pages as u64 * PAGE_SIZE)
            .ok_or(BuildError::AddressOverflow)?;
        let stack_top = stack_base
            .checked_add(stack_pages as u64 * PAGE_SIZE)
            .ok_or(BuildError::AddressOverflow)?;

        address_space.load_flat_image(code_base, self.code)?;
        address_space.map_stack(stack_base, stack_pages)?;
        address_space.initialize_heap()?;

        let auxv = self.auxv.with_entry(code_base);
        let stack = initialize_linux_stack(
            &address_space,
            stack_base,
            stack_top,
            self.argv,
            self.envp,
            self.execfn.or(self.argv.first().copied()),
            auxv,
        )?;

        let process = ProcessBuilder::new(code_base, stack.stack_pointer)
            .address_space_root(address_space.root())
            .build();
        Ok(BuiltProcess {
            process,
            address_space,
            stack,
        })
    }
}

impl<'a, S: ProgramImageSource + ?Sized> ElfProgramBuilder<'a, S> {
    pub const fn new(executable: &'a S) -> Self {
        Self {
            executable,
            interpreter: None,
            argv: &[],
            envp: &[],
            execfn: None,
            stack_pages: UserAddressSpaceLayout::current().default_elf_stack_pages,
        }
    }

    pub fn interpreter(mut self, interpreter: &'a S) -> Self {
        self.interpreter = Some(interpreter);
        self
    }

    pub fn argv(mut self, argv: &'a [&'a str]) -> Self {
        self.argv = argv;
        self
    }

    pub fn envp(mut self, envp: &'a [&'a str]) -> Self {
        self.envp = envp;
        self
    }

    pub fn execfn(mut self, execfn: &'a str) -> Self {
        self.execfn = Some(execfn);
        self
    }

    pub fn stack_pages(mut self, stack_pages: usize) -> Self {
        self.stack_pages = stack_pages.max(1);
        self
    }

    pub fn build(self) -> Result<BuiltProcess, BuildError> {
        let executable = ElfImage::parse(self.executable)?;
        let interpreter = self.interpreter.map(ElfImage::parse).transpose()?;

        let stack_pages = self.stack_pages.max(1);
        let mut address_space = UserAddressSpace::new()?;
        let stack_top = address_space.allocate_stack(stack_pages as u64 * PAGE_SIZE)?;
        let stack_base = stack_top
            .checked_sub(stack_pages as u64 * PAGE_SIZE)
            .ok_or(BuildError::AddressOverflow)?;
        let layout = address_space.layout();

        let executable_bias = executable.load_bias(layout.pie_base);
        address_space.load_elf_image(
            self.executable,
            &executable,
            executable_bias,
            UserVmaKind::Program,
        )?;

        let interpreter_bias = if let Some(interpreter) = interpreter.as_ref() {
            let bias = interpreter.load_bias(layout.interpreter_base);
            address_space.load_elf_image(
                self.interpreter.expect("interpreter bytes must exist"),
                interpreter,
                bias,
                UserVmaKind::Interpreter,
            )?;
            Some(bias)
        } else {
            None
        };

        address_space.map_stack(stack_base, stack_pages)?;
        address_space.initialize_heap()?;

        let auxv = LinuxAuxVector {
            phdr: executable.phdr_addr(executable_bias)?,
            phent: executable.phent(),
            phnum: executable.phnum(),
            base: interpreter_bias.unwrap_or(0),
            entry: executable.entry() + executable_bias,
            ..LinuxAuxVector::new()
        };
        let stack = initialize_linux_stack(
            &address_space,
            stack_base,
            stack_top,
            self.argv,
            self.envp,
            self.execfn.or(self.argv.first().copied()),
            auxv,
        )?;

        let entry = interpreter
            .as_ref()
            .map(|image| image.entry() + interpreter_bias.unwrap_or(0))
            .unwrap_or(executable.entry() + executable_bias);
        let process = ProcessBuilder::new(entry, stack.stack_pointer)
            .address_space_root(address_space.root())
            .build();
        Ok(BuiltProcess {
            process,
            address_space,
            stack,
        })
    }
}

pub fn elf_interpreter_path<S: ProgramImageSource + ?Sized>(
    source: &S,
) -> Result<Option<String>, BuildError> {
    let image = ElfImage::parse(source)?;
    image.interpreter_path(source).map_err(BuildError::from)
}

#[derive(Clone)]
pub struct UserAddressSpace {
    inner: Arc<SpinLock<UserAddressSpaceInner>>,
}

impl UserAddressSpace {
    fn new() -> Result<Self, BuildError> {
        Ok(Self {
            inner: Arc::new(SpinLock::new(UserAddressSpaceInner::new()?)),
        })
    }

    pub fn share(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }

    fn initialize_heap(&mut self) -> Result<(), BuildError> {
        self.inner.lock_irqsave().initialize_heap()
    }

    pub fn stack_base(&self) -> u64 {
        self.inner.lock_irqsave().stack_base
    }

    pub fn layout(&self) -> UserAddressSpaceLayout {
        self.inner.lock_irqsave().layout
    }

    pub fn root(&self) -> PhysFrame {
        self.inner.lock_irqsave().root
    }

    pub fn fork_cow(&self) -> Result<Self, BuildError> {
        let copy = self.inner.lock_irqsave().fork_cow()?;
        Ok(Self {
            inner: Arc::new(SpinLock::new(copy)),
        })
    }

    pub fn fork_copy(&self) -> Result<Self, BuildError> {
        let copy = self.inner.lock_irqsave().fork_copy()?;
        Ok(Self {
            inner: Arc::new(SpinLock::new(copy)),
        })
    }

    pub fn brk(&mut self, new_brk: u64) -> Result<u64, BuildError> {
        self.inner.lock_irqsave().brk(new_brk)
    }

    fn allocate_region(&mut self, len: u64) -> Result<u64, BuildError> {
        self.inner.lock_irqsave().allocate_region(len)
    }

    fn allocate_stack(&mut self, len: u64) -> Result<u64, BuildError> {
        self.inner.lock_irqsave().allocate_stack(len)
    }

    pub fn mmap_anonymous(
        &mut self,
        addr: u64,
        len: u64,
        flags: u64,
        page_flags: MapFlags,
    ) -> Result<u64, BuildError> {
        self.inner
            .lock_irqsave()
            .mmap_anonymous(addr, len, flags, page_flags)
    }

    pub fn mmap_bytes(
        &mut self,
        addr: u64,
        len: u64,
        flags: u64,
        page_flags: MapFlags,
        bytes: &[u8],
    ) -> Result<u64, BuildError> {
        self.inner
            .lock_irqsave()
            .mmap_bytes(addr, len, flags, page_flags, bytes)
    }

    pub fn mmap_physical(
        &mut self,
        addr: u64,
        len: u64,
        flags: u64,
        page_flags: MapFlags,
        physical_address: u64,
    ) -> Result<u64, BuildError> {
        self.inner
            .lock_irqsave()
            .mmap_physical(addr, len, flags, page_flags, physical_address)
    }

    pub fn munmap(&mut self, addr: u64, len: u64) -> Result<(), BuildError> {
        self.inner.lock_irqsave().munmap(addr, len)
    }

    pub fn mprotect(&mut self, addr: u64, len: u64, flags: MapFlags) -> Result<(), BuildError> {
        self.inner.lock_irqsave().mprotect(addr, len, flags)
    }

    pub fn read(&self, address: u64, buffer: &mut [u8]) -> Result<usize, BuildError> {
        self.inner.lock_irqsave().read(address, buffer)
    }

    pub fn write(&self, address: u64, buffer: &[u8]) -> Result<usize, BuildError> {
        self.inner.lock_irqsave().write(address, buffer)
    }

    pub fn read_c_string(&self, address: u64, max_len: usize) -> Result<String, BuildError> {
        self.inner.lock_irqsave().read_c_string(address, max_len)
    }

    pub fn read_user_exact(&self, address: u64, len: usize) -> Result<Vec<u8>, BuildError> {
        let mut buffer = vec![0; len];
        let read = self.read(address, &mut buffer)?;
        if read != len {
            return Err(BuildError::AddressOverflow);
        }
        Ok(buffer)
    }

    fn load_flat_image<S: ProgramImageSource + ?Sized>(
        &mut self,
        base: u64,
        image: &S,
    ) -> Result<(), BuildError> {
        self.inner.lock_irqsave().load_flat_image(base, image)
    }

    fn map_stack(&mut self, stack_base: u64, stack_pages: usize) -> Result<(), BuildError> {
        self.inner.lock_irqsave().map_stack(stack_base, stack_pages)
    }

    fn load_elf_image<S: ProgramImageSource + ?Sized>(
        &mut self,
        source: &S,
        image: &ElfImage,
        load_bias: u64,
        kind: UserVmaKind,
    ) -> Result<(), BuildError> {
        self.inner
            .lock_irqsave()
            .load_elf_image(source, image, load_bias, kind)
    }

    pub fn handle_page_fault(&self, address: u64, error_code: u64) -> Result<bool, BuildError> {
        self.inner
            .lock_irqsave()
            .handle_page_fault(address, error_code)
    }
}

struct UserAddressSpaceInner {
    layout: UserAddressSpaceLayout,
    root: PhysFrame,
    stack_base: u64,
    mappings: Vec<UserMapping>,
    vmas: Vec<UserVma>,
    brk_start: u64,
    brk_current: u64,
    brk_limit: u64,
}

impl UserAddressSpaceInner {
    fn new() -> Result<Self, BuildError> {
        let layout = UserAddressSpaceLayout::current();
        Ok(Self {
            layout,
            root: new_user_root()?,
            stack_base: 0,
            mappings: Vec::new(),
            vmas: Vec::new(),
            brk_start: 0,
            brk_current: 0,
            brk_limit: 0,
        })
    }

    fn initialize_heap(&mut self) -> Result<(), BuildError> {
        let start = self.allocate_region(self.layout.brk_reserve)?;
        self.brk_start = start;
        self.brk_current = start;
        self.brk_limit = start + self.layout.brk_reserve;
        self.record_vma(
            self.brk_start,
            self.brk_current,
            MapFlags::READ | MapFlags::WRITE | MapFlags::USER,
            UserVmaKind::Heap,
            true,
        );
        Ok(())
    }

    fn fork_cow(&mut self) -> Result<Self, BuildError> {
        let mut copy = Self::new()?;
        copy.layout = self.layout;
        copy.stack_base = self.stack_base;
        copy.vmas = self.vmas.clone();
        copy.brk_start = self.brk_start;
        copy.brk_current = self.brk_current;
        copy.brk_limit = self.brk_limit;

        let mut parent_mapper = AddressSpace::<ArchitecturePageTable>::from_root(self.root);
        let mut child_mapper = AddressSpace::<ArchitecturePageTable>::from_root(copy.root);
        let mut allocator = frame_allocator().lock_irqsave();

        for index in 0..self.mappings.len() {
            let mapping_kind = self
                .vma_for_address(self.mappings[index].virt.as_u64())
                .map(|vma| vma.kind);
            let mapping = &mut self.mappings[index];
            if matches!(mapping_kind, Some(UserVmaKind::Stack)) {
                let frame = allocator.alloc(1)?;
                copy_frame(mapping.frame, frame);
                child_mapper.map(
                    mapping.virt,
                    frame,
                    MapSize::Size4KiB,
                    mapping.flags,
                    &mut *allocator,
                )?;
                copy.mappings.push(UserMapping {
                    virt: mapping.virt,
                    frame,
                    flags: mapping.flags,
                    cow: false,
                    owned: true,
                });
                continue;
            }

            if !mapping.owned {
                child_mapper.map(
                    mapping.virt,
                    mapping.frame,
                    MapSize::Size4KiB,
                    mapping.flags,
                    &mut *allocator,
                )?;
                copy.mappings.push(UserMapping {
                    virt: mapping.virt,
                    frame: mapping.frame,
                    flags: mapping.flags,
                    cow: false,
                    owned: false,
                });
                continue;
            }

            let shared = mapping.cow || mapping.flags.contains(MapFlags::WRITE | MapFlags::USER);
            let child_flags = if shared {
                mapping.flags.without(MapFlags::WRITE)
            } else {
                mapping.flags
            };

            allocator.retain(mapping.frame)?;
            child_mapper.map(
                mapping.virt,
                mapping.frame,
                MapSize::Size4KiB,
                child_flags,
                &mut *allocator,
            )?;

            if shared && !mapping.cow {
                parent_mapper.protect(mapping.virt, child_flags)?;
                mapping.flags = child_flags;
                mapping.cow = true;
            }

            copy.mappings.push(UserMapping {
                virt: mapping.virt,
                frame: mapping.frame,
                flags: child_flags,
                cow: shared,
                owned: true,
            });
        }

        Ok(copy)
    }

    fn fork_copy(&self) -> Result<Self, BuildError> {
        let mut copy = Self::new()?;
        copy.layout = self.layout;
        copy.stack_base = self.stack_base;
        copy.vmas = self.vmas.clone();
        copy.brk_start = self.brk_start;
        copy.brk_current = self.brk_current;
        copy.brk_limit = self.brk_limit;

        for mapping in &self.mappings {
            copy.copy_mapping(mapping)?;
        }

        Ok(copy)
    }

    fn brk(&mut self, new_brk: u64) -> Result<u64, BuildError> {
        if new_brk == 0 {
            return Ok(self.brk_current);
        }
        if new_brk < self.brk_start || new_brk > self.brk_limit {
            return Ok(self.brk_current);
        }

        let old_end = align_up(self.brk_current, PAGE_SIZE);
        let new_end = align_up(new_brk, PAGE_SIZE);
        if new_end < old_end {
            for page in (new_end..old_end).step_by(PAGE_SIZE as usize) {
                let _ = self.unmap_page(page);
            }
        }

        self.brk_current = new_brk;
        self.update_heap_vma_end(new_brk);
        Ok(self.brk_current)
    }

    fn mmap_anonymous(
        &mut self,
        addr: u64,
        len: u64,
        flags: u64,
        page_flags: MapFlags,
    ) -> Result<u64, BuildError> {
        if len == 0 || (flags & MAP_ANONYMOUS) == 0 {
            return Err(BuildError::AddressOverflow);
        }

        let aligned_len = align_up(len, PAGE_SIZE);
        let base = self.prepare_mapping_base(addr, aligned_len, flags)?;
        self.record_vma(
            base,
            base + aligned_len,
            page_flags,
            UserVmaKind::Anonymous,
            true,
        );
        Ok(base)
    }

    fn mmap_bytes(
        &mut self,
        addr: u64,
        len: u64,
        flags: u64,
        page_flags: MapFlags,
        bytes: &[u8],
    ) -> Result<u64, BuildError> {
        if len == 0 {
            return Err(BuildError::AddressOverflow);
        }

        let aligned_len = align_up(len, PAGE_SIZE);
        let base = self.prepare_mapping_base(addr, aligned_len, flags)?;
        for page in (0..aligned_len).step_by(PAGE_SIZE as usize) {
            let start = page as usize;
            let end = core::cmp::min(start.saturating_add(PAGE_SIZE as usize), bytes.len());
            let slice = if start < bytes.len() {
                &bytes[start..end]
            } else {
                &[]
            };
            self.map_page(base + page, page_flags, 0, slice)?;
        }

        self.record_vma(
            base,
            base + aligned_len,
            page_flags,
            UserVmaKind::Anonymous,
            false,
        );
        Ok(base)
    }

    fn mmap_physical(
        &mut self,
        addr: u64,
        len: u64,
        flags: u64,
        page_flags: MapFlags,
        physical_address: u64,
    ) -> Result<u64, BuildError> {
        if len == 0 {
            return Err(BuildError::AddressOverflow);
        }

        let page_base = physical_address & !(PAGE_SIZE - 1);
        let page_offset = physical_address.saturating_sub(page_base);
        let total_len = align_up(len.saturating_add(page_offset), PAGE_SIZE);
        let base = self.prepare_mapping_base(addr, total_len, flags)?;

        for page in (0..total_len).step_by(PAGE_SIZE as usize) {
            let frame = PhysFrame::from_start_address(PhysAddr::new(page_base + page));
            self.map_physical_page(base + page, frame, page_flags)?;
        }

        self.record_vma(
            base,
            base + total_len,
            page_flags,
            UserVmaKind::Device,
            false,
        );
        Ok(base + page_offset)
    }

    fn munmap(&mut self, addr: u64, len: u64) -> Result<(), BuildError> {
        if len == 0 || !VirtAddr::new(addr).is_aligned(PAGE_SIZE) {
            return Err(BuildError::AddressOverflow);
        }

        let aligned_len = align_up(len, PAGE_SIZE);
        for page in (0..aligned_len).step_by(PAGE_SIZE as usize) {
            let _ = self.unmap_page(addr + page);
        }
        self.remove_vma_range(addr, addr + aligned_len);
        Ok(())
    }

    fn mprotect(&mut self, addr: u64, len: u64, flags: MapFlags) -> Result<(), BuildError> {
        if len == 0 || !VirtAddr::new(addr).is_aligned(PAGE_SIZE) {
            return Err(BuildError::AddressOverflow);
        }

        let aligned_len = align_up(len, PAGE_SIZE);
        let mut changed = false;
        let mut mapper = AddressSpace::<ArchitecturePageTable>::from_root(self.root);
        for page in (0..aligned_len).step_by(PAGE_SIZE as usize) {
            let page_addr = addr + page;
            if let Some(mapping) = self.mapping_mut(page_addr) {
                mapper.protect(VirtAddr::new(page_addr), flags)?;
                mapping.flags = flags;
                mapping.cow = false;
                changed = true;
            } else if self.vma_for_address(page_addr).is_some() {
                changed = true;
            }
        }
        if !changed {
            return Err(BuildError::Map(MappingError::NotMapped));
        }
        self.update_vma_flags(addr, addr + aligned_len, flags);
        Ok(())
    }

    fn read(&mut self, address: u64, buffer: &mut [u8]) -> Result<usize, BuildError> {
        let mut copied = 0usize;
        let mut current = address;

        while copied < buffer.len() {
            self.ensure_access(current, false)?;
            let Some((page, offset)) = self.translate(current) else {
                break;
            };
            let chunk = core::cmp::min(buffer.len() - copied, PAGE_SIZE as usize - offset);
            unsafe {
                core::ptr::copy_nonoverlapping(
                    page.add(offset),
                    buffer[copied..copied + chunk].as_mut_ptr(),
                    chunk,
                );
            }
            copied += chunk;
            current = current
                .checked_add(chunk as u64)
                .ok_or(BuildError::AddressOverflow)?;
        }

        Ok(copied)
    }

    fn write(&mut self, address: u64, buffer: &[u8]) -> Result<usize, BuildError> {
        let mut copied = 0usize;
        let mut current = address;

        while copied < buffer.len() {
            self.ensure_access(current, true)?;
            let Some((page, offset)) = self.translate(current) else {
                break;
            };
            let chunk = core::cmp::min(buffer.len() - copied, PAGE_SIZE as usize - offset);
            unsafe {
                core::ptr::copy_nonoverlapping(
                    buffer[copied..copied + chunk].as_ptr(),
                    page.add(offset),
                    chunk,
                );
            }
            copied += chunk;
            current = current
                .checked_add(chunk as u64)
                .ok_or(BuildError::AddressOverflow)?;
        }

        Ok(copied)
    }

    fn read_c_string(&mut self, address: u64, max_len: usize) -> Result<String, BuildError> {
        let mut bytes = Vec::new();
        let mut current = address;

        while bytes.len() < max_len {
            self.ensure_access(current, false)?;
            let Some((page, offset)) = self.translate(current) else {
                break;
            };
            let byte = unsafe { page.add(offset).read() };
            if byte == 0 {
                return String::from_utf8(bytes).map_err(|_| BuildError::AddressOverflow);
            }
            bytes.push(byte);
            current = current.checked_add(1).ok_or(BuildError::AddressOverflow)?;
        }

        String::from_utf8(bytes).map_err(|_| BuildError::AddressOverflow)
    }

    fn load_flat_image<S: ProgramImageSource + ?Sized>(
        &mut self,
        base: u64,
        image: &S,
    ) -> Result<(), BuildError> {
        let flags = MapFlags::READ | MapFlags::EXECUTE | MapFlags::USER;
        let aligned_len = align_up(image.len() as u64, PAGE_SIZE);
        let mut page_buffer = vec![0u8; PAGE_SIZE as usize];
        for page_offset in (0..aligned_len).step_by(PAGE_SIZE as usize) {
            let offset = page_offset as usize;
            let len = core::cmp::min(PAGE_SIZE as usize, image.len().saturating_sub(offset));
            let slice = if len == 0 {
                &[]
            } else {
                image.read_exact_at(offset, &mut page_buffer[..len])?;
                &page_buffer[..len]
            };
            self.map_page(base + page_offset, flags, 0, slice)?;
        }
        self.record_vma(base, base + aligned_len, flags, UserVmaKind::Program, false);
        Ok(())
    }

    fn map_stack(&mut self, stack_base: u64, stack_pages: usize) -> Result<(), BuildError> {
        let flags = MapFlags::READ | MapFlags::WRITE | MapFlags::USER;
        self.stack_base = stack_base;
        self.record_vma(
            stack_base,
            stack_base + stack_pages as u64 * PAGE_SIZE,
            flags,
            UserVmaKind::Stack,
            true,
        );
        Ok(())
    }

    fn load_elf_image<S: ProgramImageSource + ?Sized>(
        &mut self,
        source: &S,
        image: &ElfImage,
        load_bias: u64,
        kind: UserVmaKind,
    ) -> Result<(), BuildError> {
        let mut page_buffer = vec![0u8; PAGE_SIZE as usize];
        for header in image.program_headers() {
            if header.kind != ElfSegmentType::Load {
                continue;
            }

            let segment_base = load_bias + header.virtual_addr;
            let map_base = align_down(segment_base, PAGE_SIZE);
            let map_end = align_up(segment_base + header.mem_size, PAGE_SIZE);
            let flags = map_program_flags(header.flags);
            for page_base in (map_base..map_end).step_by(PAGE_SIZE as usize) {
                self.ensure_page_flags(page_base, flags)?;
                let page_end = page_base
                    .checked_add(PAGE_SIZE)
                    .ok_or(BuildError::AddressOverflow)?;
                let file_segment_end = segment_base
                    .checked_add(header.file_size)
                    .ok_or(BuildError::AddressOverflow)?;
                let copy_start = page_base.max(segment_base);
                let copy_end = page_end.min(file_segment_end);
                if copy_start >= copy_end {
                    continue;
                }

                let dest_offset = (copy_start - page_base) as usize;
                let source_offset = header
                    .offset
                    .checked_add(copy_start - segment_base)
                    .ok_or(BuildError::AddressOverflow)?;
                let len = (copy_end - copy_start) as usize;
                source.read_exact_at(source_offset as usize, &mut page_buffer[..len])?;
                self.write_mapped_page(page_base, dest_offset, &page_buffer[..len])?;
            }
            self.record_vma(map_base, map_end, flags, kind, false);
        }
        Ok(())
    }

    fn handle_page_fault(&mut self, address: u64, error_code: u64) -> Result<bool, BuildError> {
        let page_base = align_down(address, PAGE_SIZE);
        let write_fault = (error_code & 0x2) != 0;

        if let Some(mapping) = self.mapping(page_base) {
            if write_fault && mapping.cow {
                self.resolve_cow_fault(page_base)?;
                return Ok(true);
            }
            return Ok(false);
        }

        self.map_lazy_page(page_base, write_fault)
    }

    fn ensure_access(&mut self, address: u64, write: bool) -> Result<(), BuildError> {
        let page_base = align_down(address, PAGE_SIZE);
        if let Some(mapping) = self.mapping(page_base) {
            if write {
                if mapping.cow {
                    self.resolve_cow_fault(page_base)?;
                    return Ok(());
                }
                if !mapping.flags.contains(MapFlags::WRITE) {
                    return Err(BuildError::Map(MappingError::NotMapped));
                }
            }
            return Ok(());
        }

        if self.map_lazy_page(page_base, write)? {
            return Ok(());
        }

        Err(BuildError::Map(MappingError::NotMapped))
    }

    fn map_lazy_page(&mut self, page_base: u64, write: bool) -> Result<bool, BuildError> {
        let Some(vma) = self.vma_for_address(page_base).cloned() else {
            return Ok(false);
        };
        if !vma.lazy {
            return Ok(false);
        }
        if write && !vma.flags.contains(MapFlags::WRITE) {
            return Ok(false);
        }
        self.map_zeroed_page(page_base, vma.flags)?;
        Ok(true)
    }

    fn resolve_cow_fault(&mut self, page_base: u64) -> Result<(), BuildError> {
        let index = self
            .mapping_index(page_base)
            .ok_or(BuildError::Map(MappingError::NotMapped))?;
        let old_frame = self.mappings[index].frame;
        let writable_flags = self.mappings[index].flags | MapFlags::WRITE;

        let mut mapper = AddressSpace::<ArchitecturePageTable>::from_root(self.root);
        let mut allocator = frame_allocator().lock_irqsave();
        let ref_count = allocator.ref_count(old_frame).unwrap_or(1);
        if ref_count > 1 {
            let new_frame = allocator.alloc(1)?;
            copy_frame(old_frame, new_frame);
            let _ = mapper.unmap(VirtAddr::new(page_base), &mut *allocator)?;
            mapper.map(
                VirtAddr::new(page_base),
                new_frame,
                MapSize::Size4KiB,
                writable_flags,
                &mut *allocator,
            )?;
            self.mappings[index].frame = new_frame;
        } else {
            mapper.protect(VirtAddr::new(page_base), writable_flags)?;
        }

        self.mappings[index].flags = writable_flags;
        self.mappings[index].cow = false;
        Ok(())
    }

    fn map_zeroed_page(&mut self, addr: u64, flags: MapFlags) -> Result<(), BuildError> {
        let mut mapper = AddressSpace::<ArchitecturePageTable>::from_root(self.root);
        let mut allocator = frame_allocator().lock_irqsave();
        let frame = allocator.alloc(1)?;
        zero_frame(frame);
        let virt = VirtAddr::new(addr);
        mapper.map(virt, frame, MapSize::Size4KiB, flags, &mut *allocator)?;
        self.insert_mapping(UserMapping {
            virt,
            frame,
            flags,
            cow: false,
            owned: true,
        });
        Ok(())
    }

    fn ensure_page_flags(&mut self, addr: u64, flags: MapFlags) -> Result<(), BuildError> {
        if let Some(current_flags) = self.mapping(addr).map(|mapping| mapping.flags) {
            let merged_flags = current_flags | flags;
            if merged_flags != current_flags {
                let mut mapper = AddressSpace::<ArchitecturePageTable>::from_root(self.root);
                mapper.protect(VirtAddr::new(addr), merged_flags)?;
                if let Some(mapping) = self.mapping_mut(addr) {
                    mapping.flags = merged_flags;
                    mapping.cow = false;
                }
            }
            return Ok(());
        }

        self.map_zeroed_page(addr, flags)
    }

    fn write_mapped_page(
        &mut self,
        addr: u64,
        dest_offset: usize,
        bytes: &[u8],
    ) -> Result<(), BuildError> {
        let frame = self
            .mapping(addr)
            .map(|mapping| mapping.frame)
            .ok_or(BuildError::Map(MappingError::NotMapped))?;
        write_frame_partial(frame, dest_offset, bytes);
        Ok(())
    }

    fn map_page(
        &mut self,
        addr: u64,
        flags: MapFlags,
        dest_offset: usize,
        bytes: &[u8],
    ) -> Result<(), BuildError> {
        let mut mapper = AddressSpace::<ArchitecturePageTable>::from_root(self.root);
        let mut allocator = frame_allocator().lock_irqsave();
        let frame = allocator.alloc(1)?;
        zero_frame(frame);
        if !bytes.is_empty() {
            write_frame_partial(frame, dest_offset, bytes);
        }
        let virt = VirtAddr::new(addr);
        mapper.map(virt, frame, MapSize::Size4KiB, flags, &mut *allocator)?;
        self.insert_mapping(UserMapping {
            virt,
            frame,
            flags,
            cow: false,
            owned: true,
        });
        Ok(())
    }

    fn map_physical_page(
        &mut self,
        addr: u64,
        frame: PhysFrame,
        flags: MapFlags,
    ) -> Result<(), BuildError> {
        let mut mapper = AddressSpace::<ArchitecturePageTable>::from_root(self.root);
        let mut allocator = frame_allocator().lock_irqsave();
        let virt = VirtAddr::new(addr);
        mapper.map(virt, frame, MapSize::Size4KiB, flags, &mut *allocator)?;
        self.insert_mapping(UserMapping {
            virt,
            frame,
            flags,
            cow: false,
            owned: false,
        });
        Ok(())
    }

    fn copy_mapping(&mut self, mapping: &UserMapping) -> Result<(), BuildError> {
        let mut allocator = frame_allocator().lock_irqsave();
        let frame = allocator.alloc(1)?;
        copy_frame(mapping.frame, frame);

        let virt = mapping.virt;
        let mut mapper = AddressSpace::<ArchitecturePageTable>::from_root(self.root);
        mapper.map(
            virt,
            frame,
            MapSize::Size4KiB,
            mapping.flags
                | if mapping.cow {
                    MapFlags::WRITE
                } else {
                    MapFlags::empty()
                },
            &mut *allocator,
        )?;
        self.insert_mapping(UserMapping {
            virt,
            frame,
            flags: if mapping.cow {
                mapping.flags | MapFlags::WRITE
            } else {
                mapping.flags
            },
            cow: false,
            owned: true,
        });
        Ok(())
    }

    fn unmap_page(&mut self, addr: u64) -> Result<(), BuildError> {
        let index = self
            .mapping_index(addr)
            .ok_or(BuildError::Map(MappingError::NotMapped))?;
        let mapping = self.mappings.remove(index);
        let virt = mapping.virt;
        let mut mapper = AddressSpace::<ArchitecturePageTable>::from_root(self.root);
        let mut allocator = frame_allocator().lock_irqsave();
        if mapping.owned {
            let _ = mapper.unmap(virt, &mut *allocator)?;
        } else {
            let _ = mapper.unmap_preserve(virt, &mut *allocator)?;
        }
        Ok(())
    }

    fn prepare_mapping_base(&mut self, addr: u64, len: u64, flags: u64) -> Result<u64, BuildError> {
        if (flags & MAP_FIXED) != 0 {
            if !VirtAddr::new(addr).is_aligned(PAGE_SIZE) {
                return Err(BuildError::AddressOverflow);
            }
            let _ = self.munmap(addr, len);
            Ok(addr)
        } else {
            self.allocate_mmap_base(len)
        }
    }

    fn allocate_mmap_base(&mut self, len: u64) -> Result<u64, BuildError> {
        let lower_bound = align_up(self.brk_limit, PAGE_SIZE);
        let upper_bound = align_down(self.layout.mmap_top, PAGE_SIZE);
        self.find_top_down_gap(lower_bound, upper_bound, len)
            .ok_or(BuildError::Frame(
                aether_frame::mm::FrameAllocError::OutOfMemory,
            ))
    }

    fn allocate_region(&mut self, len: u64) -> Result<u64, BuildError> {
        let len = align_up(len, self.layout.region_align);
        let current = self.layout.region_base;
        let next = current
            .checked_add(len)
            .ok_or(BuildError::AddressOverflow)?;
        self.layout.region_base = next;
        Ok(current)
    }

    fn allocate_stack(&mut self, len: u64) -> Result<u64, BuildError> {
        let len = align_up(len, self.layout.region_align);
        let top = self.layout.stack_end;
        let next = top.checked_sub(len).ok_or(BuildError::AddressOverflow)?;
        self.layout.stack_end = next;
        Ok(top)
    }

    fn translate(&self, address: u64) -> Option<(*mut u8, usize)> {
        let page_base = address & !(PAGE_SIZE - 1);
        let offset = (address - page_base) as usize;
        let frame = self.mapping(page_base).map(|mapping| mapping.frame)?;
        Some((
            phys_to_virt(frame.start_address().as_u64()) as *mut u8,
            offset,
        ))
    }

    fn mapping(&self, address: u64) -> Option<&UserMapping> {
        self.mapping_index(address)
            .map(|index| &self.mappings[index])
    }

    fn mapping_mut(&mut self, address: u64) -> Option<&mut UserMapping> {
        self.mapping_index(address)
            .map(|index| &mut self.mappings[index])
    }

    fn mapping_index(&self, address: u64) -> Option<usize> {
        let page_base = align_down(address, PAGE_SIZE);
        self.mappings
            .binary_search_by_key(&page_base, |mapping| mapping.virt.as_u64())
            .ok()
    }

    fn insert_mapping(&mut self, mapping: UserMapping) {
        let addr = mapping.virt.as_u64();
        let index = self
            .mappings
            .binary_search_by_key(&addr, |existing| existing.virt.as_u64())
            .unwrap_or_else(|index| index);
        self.mappings.insert(index, mapping);
    }

    fn record_vma(&mut self, start: u64, end: u64, flags: MapFlags, kind: UserVmaKind, lazy: bool) {
        let vma = UserVma {
            start,
            end,
            flags,
            kind,
            lazy,
        };
        let index = self
            .vmas
            .binary_search_by_key(&start, |existing| existing.start)
            .unwrap_or_else(|index| index);
        self.vmas.insert(index, vma);
    }

    fn vma_for_address(&self, address: u64) -> Option<&UserVma> {
        self.vmas
            .iter()
            .find(|vma| address >= vma.start && address < vma.end)
    }

    fn update_heap_vma_end(&mut self, end: u64) {
        if let Some(vma) = self
            .vmas
            .iter_mut()
            .find(|vma| matches!(vma.kind, UserVmaKind::Heap))
        {
            vma.end = end;
        }
    }

    fn update_vma_flags(&mut self, start: u64, end: u64, flags: MapFlags) {
        for vma in &mut self.vmas {
            if vma.end <= start || vma.start >= end {
                continue;
            }
            vma.flags = flags;
        }
    }

    fn remove_vma_range(&mut self, start: u64, end: u64) {
        let mut updated = Vec::with_capacity(self.vmas.len());
        for vma in self.vmas.drain(..) {
            if end <= vma.start || start >= vma.end {
                updated.push(vma);
                continue;
            }
            if start > vma.start {
                updated.push(UserVma {
                    start: vma.start,
                    end: start,
                    ..vma.clone()
                });
            }
            if end < vma.end {
                updated.push(UserVma {
                    start: end,
                    end: vma.end,
                    ..vma
                });
            }
        }
        self.vmas = updated;
    }

    fn find_top_down_gap(&self, lower_bound: u64, upper_bound: u64, len: u64) -> Option<u64> {
        let aligned_len = align_up(len, PAGE_SIZE);
        if upper_bound < lower_bound || upper_bound.checked_sub(lower_bound)? < aligned_len {
            return None;
        }

        let mut gap_end = upper_bound;
        for vma in self.vmas.iter().rev() {
            if vma.start >= upper_bound {
                continue;
            }
            if vma.end <= lower_bound {
                break;
            }

            if let Some(base) = gap_end
                .checked_sub(aligned_len)
                .map(|candidate| align_down(candidate, PAGE_SIZE))
                .filter(|candidate| {
                    *candidate >= lower_bound && candidate.saturating_add(aligned_len) <= vma.start
                })
            {
                return Some(base);
            }

            gap_end = align_down(vma.start, PAGE_SIZE);
            if gap_end <= lower_bound {
                return None;
            }
        }

        gap_end
            .checked_sub(aligned_len)
            .map(|candidate| align_down(candidate, PAGE_SIZE))
            .filter(|candidate| *candidate >= lower_bound)
    }
}

impl Drop for UserAddressSpaceInner {
    fn drop(&mut self) {
        let mut mapper = AddressSpace::<ArchitecturePageTable>::from_root(self.root);
        let mut allocator = frame_allocator().lock_irqsave();

        for mapping in self.mappings.drain(..).rev() {
            let virt = mapping.virt;
            if mapping.owned {
                let _ = mapper.unmap(virt, &mut *allocator);
            } else {
                let _ = mapper.unmap_preserve(virt, &mut *allocator);
            }
        }
    }
}

struct UserMapping {
    virt: VirtAddr,
    frame: PhysFrame,
    flags: MapFlags,
    cow: bool,
    owned: bool,
}

#[derive(Clone, Copy)]
enum UserVmaKind {
    Program,
    Interpreter,
    Heap,
    Stack,
    Anonymous,
    Device,
}

#[derive(Clone)]
struct UserVma {
    start: u64,
    end: u64,
    flags: MapFlags,
    kind: UserVmaKind,
    lazy: bool,
}

fn initialize_linux_stack(
    address_space: &UserAddressSpace,
    stack_base: u64,
    stack_top: u64,
    argv: &[&str],
    envp: &[&str],
    execfn: Option<&str>,
    auxv: LinuxAuxVector,
) -> Result<LinuxStackInfo, BuildError> {
    let execfn_name = execfn.unwrap_or(DEFAULT_EXECFN);
    let mut writer = StackWriter::new(address_space, stack_base, stack_top);

    let execfn_ptr = push_c_string(&mut writer, execfn_name.as_bytes())?;
    let platform_ptr = arch_platform_string()
        .map(|platform| push_c_string(&mut writer, platform))
        .transpose()?
        .unwrap_or(0);
    let random_values = next_aux_random();
    let random_ptr = writer.push_raw_bytes(&random_values_as_bytes(&random_values))?;

    let mut env_addrs = Vec::with_capacity(envp.len());
    let mut env_low = u64::MAX;
    let mut env_high = 0;
    for entry in envp.iter().rev() {
        let ptr = push_c_string(&mut writer, entry.as_bytes())?;
        env_addrs.push(ptr);
        env_low = env_low.min(ptr);
        env_high = env_high.max(ptr + entry.len() as u64 + 1);
    }
    env_addrs.reverse();

    let mut argv_addrs = Vec::with_capacity(argv.len());
    let mut arg_low = u64::MAX;
    let mut arg_high = 0;
    for entry in argv.iter().rev() {
        let ptr = push_c_string(&mut writer, entry.as_bytes())?;
        argv_addrs.push(ptr);
        arg_low = arg_low.min(ptr);
        arg_high = arg_high.max(ptr + entry.len() as u64 + 1);
    }
    argv_addrs.reverse();

    let aux_entries = [
        AuxEntry {
            key: AT_EXECFN,
            value: execfn_ptr,
        },
        AuxEntry {
            key: AT_RANDOM,
            value: random_ptr,
        },
        AuxEntry {
            key: AT_BASE_PLATFORM,
            value: 0,
        },
        AuxEntry {
            key: AT_EGID,
            value: auxv.egid,
        },
        AuxEntry {
            key: AT_GID,
            value: auxv.gid,
        },
        AuxEntry {
            key: AT_EUID,
            value: auxv.euid,
        },
        AuxEntry {
            key: AT_UID,
            value: auxv.uid,
        },
        AuxEntry {
            key: AT_ENTRY,
            value: auxv.entry,
        },
        AuxEntry {
            key: AT_MINSIGSTKSZ,
            value: auxv.minsigstksz,
        },
        AuxEntry {
            key: AT_FLAGS,
            value: auxv.flags,
        },
        AuxEntry {
            key: AT_HWCAP,
            value: auxv.hwcap,
        },
        AuxEntry {
            key: AT_HWCAP2,
            value: auxv.hwcap2,
        },
        AuxEntry {
            key: AT_PLATFORM,
            value: platform_ptr,
        },
        AuxEntry {
            key: AT_SECURE,
            value: auxv.secure,
        },
        AuxEntry {
            key: AT_PHNUM,
            value: auxv.phnum,
        },
        AuxEntry {
            key: AT_PHENT,
            value: auxv.phent,
        },
        AuxEntry {
            key: AT_PHDR,
            value: auxv.phdr,
        },
        AuxEntry {
            key: AT_PAGESZ,
            value: auxv.page_size,
        },
        AuxEntry {
            key: AT_CLKTCK,
            value: auxv.clock_tick,
        },
        AuxEntry {
            key: AT_BASE,
            value: auxv.base,
        },
        AuxEntry {
            key: AT_SYSINFO_EHDR,
            value: auxv.sysinfo_ehdr,
        },
    ];

    let qwords_to_push = aux_entries.len() * 2 + argv_addrs.len() + env_addrs.len() + 3;
    writer.align_down(16);
    if !qwords_to_push.is_multiple_of(2) {
        writer.push_u64(0)?;
    }

    writer.push_u64(0)?;
    writer.push_u64(AT_NULL)?;

    for entry in &aux_entries {
        writer.push_u64(entry.value)?;
        writer.push_u64(entry.key)?;
    }

    writer.push_u64(0)?;
    for addr in env_addrs.iter().rev() {
        writer.push_u64(*addr)?;
    }

    writer.push_u64(0)?;
    for addr in argv_addrs.iter().rev() {
        writer.push_u64(*addr)?;
    }

    writer.push_u64(argv_addrs.len() as u64)?;

    Ok(LinuxStackInfo {
        stack_pointer: writer.cursor(),
        arg_start: if argv_addrs.is_empty() { 0 } else { arg_low },
        arg_end: if argv_addrs.is_empty() { 0 } else { arg_high },
        env_start: if env_addrs.is_empty() { 0 } else { env_low },
        env_end: if env_addrs.is_empty() { 0 } else { env_high },
        execfn_ptr,
        random_ptr,
    })
}

fn push_c_string(writer: &mut StackWriter, bytes: &[u8]) -> Result<u64, BuildError> {
    writer.push_u8(0)?;
    writer.push_bytes(bytes)
}

fn map_program_flags(flags: ElfSegmentFlags) -> MapFlags {
    let mut map = MapFlags::READ | MapFlags::USER;
    if flags.is_write() {
        map = map | MapFlags::WRITE;
    }
    if flags.is_execute() {
        map = map | MapFlags::EXECUTE;
    }
    map
}

struct StackWriter<'a> {
    address_space: &'a UserAddressSpace,
    base: u64,
    cursor: u64,
}

impl StackWriter<'_> {
    fn new(address_space: &UserAddressSpace, base: u64, top: u64) -> StackWriter<'_> {
        StackWriter {
            address_space,
            base,
            cursor: top,
        }
    }

    fn cursor(&self) -> u64 {
        self.cursor
    }

    fn align_down(&mut self, align: u64) {
        self.cursor &= !(align - 1);
    }

    fn push_u64(&mut self, value: u64) -> Result<u64, BuildError> {
        self.push_raw_bytes(&value.to_ne_bytes())
    }

    fn push_u8(&mut self, value: u8) -> Result<u64, BuildError> {
        self.push_raw_bytes(&[value])
    }

    fn push_bytes(&mut self, bytes: &[u8]) -> Result<u64, BuildError> {
        self.push_raw_bytes(bytes)
    }

    fn push_raw_bytes(&mut self, bytes: &[u8]) -> Result<u64, BuildError> {
        self.cursor = self
            .cursor
            .checked_sub(bytes.len() as u64)
            .ok_or(BuildError::AddressOverflow)?;
        if self.cursor < self.base {
            return Err(BuildError::StackOverflow);
        }

        let written = self
            .address_space
            .write(self.cursor, bytes)
            .map_err(|_| BuildError::AddressOverflow)?;
        if written != bytes.len() {
            return Err(BuildError::AddressOverflow);
        }
        Ok(self.cursor)
    }
}

fn pages_for(len: usize) -> usize {
    len.div_ceil(PAGE_SIZE as usize)
}

fn arch_platform_string() -> Option<&'static [u8]> {
    #[cfg(target_arch = "x86_64")]
    {
        return Some(b"x86_64");
    }

    #[allow(unreachable_code)]
    None
}

fn align_up(value: u64, align: u64) -> u64 {
    (value + align - 1) & !(align - 1)
}

fn align_down(value: u64, align: u64) -> u64 {
    value & !(align - 1)
}

fn zero_frame(frame: PhysFrame) {
    unsafe {
        core::ptr::write_bytes(
            phys_to_virt(frame.start_address().as_u64()) as *mut u8,
            0,
            PAGE_SIZE as usize,
        );
    }
}

fn write_frame_partial(frame: PhysFrame, offset: usize, data: &[u8]) {
    unsafe {
        core::ptr::copy_nonoverlapping(
            data.as_ptr(),
            (phys_to_virt(frame.start_address().as_u64()) as *mut u8).add(offset),
            data.len(),
        );
    }
}

fn copy_frame(source: PhysFrame, destination: PhysFrame) {
    unsafe {
        core::ptr::copy_nonoverlapping(
            phys_to_virt(source.start_address().as_u64()) as *const u8,
            phys_to_virt(destination.start_address().as_u64()) as *mut u8,
            PAGE_SIZE as usize,
        );
    }
}

fn next_aux_random() -> [u64; 2] {
    let ticks = timer::ticks();
    let mut current = AUX_RANDOM_SEED.load(Ordering::Acquire) ^ ticks.rotate_left(17);
    let first = loop {
        let next = mix64(current);
        match AUX_RANDOM_SEED.compare_exchange(current, next, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => break next,
            Err(observed) => current = observed,
        }
    };
    [first, mix64(first ^ size_of::<[u64; 2]>() as u64)]
}

fn random_values_as_bytes(values: &[u64; 2]) -> [u8; 16] {
    let mut bytes = [0; 16];
    bytes[..8].copy_from_slice(&values[0].to_ne_bytes());
    bytes[8..].copy_from_slice(&values[1].to_ne_bytes());
    bytes
}

fn mix64(mut value: u64) -> u64 {
    value ^= value >> 30;
    value = value.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}
