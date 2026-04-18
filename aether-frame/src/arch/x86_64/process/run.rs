use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use core::arch::global_asm;
use core::mem::MaybeUninit;
use core::mem::offset_of;
use core::ptr;

use x86_64::registers::model_specific::Msr;

use crate::boot::MAX_CPUS;
use crate::interrupt::{PrivilegeLevel, Trap, TrapKind};
use crate::libs::percpu::PerCpuPtr;
use crate::mm::{PageTableArch, PhysFrame};
use crate::process::{RunFuture, RunReason, RunResult};

use super::super::fpu::FpuState;
use super::context::{GeneralRegs, UserContext};

const DEFAULT_KERNEL_STACK_SIZE: usize = 64 * 1024;
const IA32_FS_BASE: u32 = 0xc000_0100;
const IA32_GS_BASE: u32 = 0xc000_0101;

#[repr(C)]
pub struct CurrentRun {
    pub kernel_rsp: u64,
    pub kernel_cr3: u64,
    pub kernel_interrupts_enabled: u64,
    pub process: usize,
    pub saved_rbx: u64,
    pub saved_rbp: u64,
    pub saved_r12: u64,
    pub saved_r13: u64,
    pub saved_r14: u64,
    pub saved_r15: u64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct KernelContext {
    pub rip: u64,
    pub rsp: u64,
    pub interrupts_enabled: u64,
    pub saved_rbx: u64,
    pub saved_rbp: u64,
    pub saved_r12: u64,
    pub saved_r13: u64,
    pub saved_r14: u64,
    pub saved_r15: u64,
}

impl Default for KernelContext {
    fn default() -> Self {
        Self {
            rip: 0,
            rsp: 0,
            interrupts_enabled: 1,
            saved_rbx: 0,
            saved_rbp: 0,
            saved_r12: 0,
            saved_r13: 0,
            saved_r14: 0,
            saved_r15: 0,
        }
    }
}

pub type KernelContextEntry = unsafe extern "C" fn(*mut ()) -> !;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct KernelContextStartFrame {
    arg: *mut (),
    entry: KernelContextEntry,
}

static CURRENT_RUNS: PerCpuPtr<CurrentRun, MAX_CPUS> = PerCpuPtr::new();
static CURRENT_SCHEDULER_CONTEXTS: PerCpuPtr<KernelContext, MAX_CPUS> = PerCpuPtr::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResumeMode {
    Iret,
    #[allow(dead_code)]
    Sysret,
}

pub struct ProcessBuilder {
    entry: u64,
    user_stack_top: u64,
    address_space_root: PhysFrame,
    kernel_stack_size: usize,
}

impl ProcessBuilder {
    #[must_use]
    pub fn new(entry: u64, user_stack_top: u64) -> Self {
        Self {
            entry,
            user_stack_top,
            address_space_root:
                <crate::arch::mm::ArchitecturePageTable as PageTableArch>::root_frame(),
            kernel_stack_size: DEFAULT_KERNEL_STACK_SIZE,
        }
    }

    #[must_use]
    pub const fn address_space_root(mut self, address_space_root: PhysFrame) -> Self {
        self.address_space_root = address_space_root;
        self
    }

    #[must_use]
    pub fn kernel_stack_size(mut self, kernel_stack_size: usize) -> Self {
        self.kernel_stack_size = kernel_stack_size.max(4096);
        self
    }

    #[must_use]
    pub fn build(self) -> Process {
        Process::new(&self)
    }
}

pub struct Process {
    context: UserContext,
    fpu_state: Box<FpuState>,
    address_space_root: PhysFrame,
    _kernel_stack: Box<[u64]>,
    kernel_stack_top: u64,
    last_reason: Option<RunReason>,
}

impl Process {
    fn new(builder: &ProcessBuilder) -> Self {
        let words = builder
            .kernel_stack_size
            .div_ceil(core::mem::size_of::<u64>());
        let mut kernel_stack: Vec<u64> = vec![0; words];
        let stack_top = align_down(
            kernel_stack.as_mut_ptr() as u64 + kernel_stack.len() as u64 * 8,
            16,
        );

        Self {
            context: UserContext::new(builder.entry, builder.user_stack_top),
            fpu_state: Box::default(),
            address_space_root: builder.address_space_root,
            _kernel_stack: kernel_stack.into_boxed_slice(),
            kernel_stack_top: stack_top,
            last_reason: None,
        }
    }

    #[must_use]
    pub const fn context(&self) -> &UserContext {
        &self.context
    }

    #[must_use]
    pub const fn context_mut(&mut self) -> &mut UserContext {
        &mut self.context
    }

    #[must_use]
    pub const fn kernel_stack_top(&self) -> u64 {
        self.kernel_stack_top
    }

    #[must_use]
    pub fn fork_with_root(&self, address_space_root: PhysFrame) -> Self {
        let mut process = ProcessBuilder::new(self.context.general.rip, self.context.general.rsp)
            .address_space_root(address_space_root)
            .kernel_stack_size(self._kernel_stack.len() * core::mem::size_of::<u64>())
            .build();
        process.context = self.context;
        process.context.general.rax = 0;
        process.fpu_state.copy_from(&self.fpu_state);
        process
    }

    #[must_use]
    pub const fn address_space_root(&self) -> PhysFrame {
        self.address_space_root
    }

    pub fn run(&mut self) -> RunResult {
        self.last_reason = None;
        let cpu_index = crate::arch::cpu::current_cpu_index();

        let mut current_run = CurrentRun {
            kernel_rsp: 0,
            kernel_cr3: <crate::arch::mm::ArchitecturePageTable as PageTableArch>::root_frame()
                .start_address()
                .as_u64(),
            kernel_interrupts_enabled: 1,
            process: core::ptr::from_mut(self) as usize,
            saved_rbx: 0,
            saved_rbp: 0,
            saved_r12: 0,
            saved_r13: 0,
            saved_r14: 0,
            saved_r15: 0,
        };

        crate::arch::interrupt::install_process_kernel_stack(self.kernel_stack_top);
        crate::arch::fpu::restore(&self.fpu_state);
        restore_user_tls_bases(&self.context);

        unsafe {
            let _ = CURRENT_RUNS.store(cpu_index, &raw mut current_run);
            match resume_mode_for_context(&self.context) {
                ResumeMode::Iret => {
                    aether_x86_enter_user_iret(
                        &raw const self.context,
                        self.address_space_root.start_address().as_u64(),
                        (&raw mut current_run).cast::<()>(),
                    );
                }
                ResumeMode::Sysret => {
                    aether_x86_enter_user_sysret(
                        &raw const self.context,
                        self.address_space_root.start_address().as_u64(),
                        (&raw mut current_run).cast::<()>(),
                    );
                }
            }
            let _ = CURRENT_RUNS.store(cpu_index, ptr::null_mut());
        }

        RunResult {
            reason: self
                .last_reason
                .expect("process returned to kernel without a recorded reason"),
            context: self.context,
        }
    }

    #[must_use]
    pub const fn run_async(&mut self) -> RunFuture<'_> {
        RunFuture::new(self)
    }
}

pub fn prepare_trap(trap: Trap) {
    if trap.privilege() != PrivilegeLevel::User {
        return;
    }

    let current_run = unsafe { current_run_for_current_cpu().as_mut() };
    let Some(current_run) = current_run else {
        return;
    };
    let process = unsafe { &mut *(current_run.process as *mut Process) };
    crate::arch::fpu::save(&mut process.fpu_state);
}

#[must_use]
pub fn on_trap(trap: Trap, frame: &crate::arch::interrupt::TrapFrame) -> Option<RunReason> {
    if trap.privilege() != PrivilegeLevel::User {
        return None;
    }

    let current_run = unsafe { current_run_for_current_cpu().as_mut()? };
    let process = unsafe { &mut *(current_run.process as *mut Process) };
    process.context.capture_from_trap(frame);

    if trap.kind() == TrapKind::Interrupt && !crate::preempt::need_resched() {
        return None;
    }

    let fault_address = super::fault_address_for_trap(trap);
    let reason = RunReason::from_trap(trap, fault_address);
    process.last_reason = Some(reason);
    Some(reason)
}

pub fn resume_current_user_context() {
    let Some(current_run) = (unsafe { current_run_for_current_cpu().as_mut() }) else {
        return;
    };
    let process = unsafe { &mut *(current_run.process as *mut Process) };
    crate::arch::fpu::restore(&process.fpu_state);
    restore_user_tls_bases(&process.context);
}

pub fn current_run_for_current_cpu() -> *mut CurrentRun {
    CURRENT_RUNS
        .load(crate::arch::cpu::current_cpu_index())
        .unwrap_or(ptr::null_mut())
}

pub fn run_on_kernel_stack<R, F>(stack_top: u64, f: F) -> R
where
    F: FnOnce() -> R,
{
    struct StackCall<F, R> {
        func: Option<F>,
        result: MaybeUninit<R>,
    }

    unsafe extern "C" fn trampoline<F, R>(arg: usize) -> usize
    where
        F: FnOnce() -> R,
    {
        let call = unsafe { &mut *(arg as *mut StackCall<F, R>) };
        let result = (call
            .func
            .take()
            .expect("kernel stack trampoline invoked twice"))();
        let _ = call.result.write(result);
        0
    }

    let mut call = StackCall {
        func: Some(f),
        result: MaybeUninit::uninit(),
    };

    unsafe {
        let _ = aether_x86_call_on_stack(
            stack_top,
            trampoline::<F, R> as *const () as usize,
            (&raw mut call).cast::<()>() as usize,
        );
        call.result.assume_init()
    }
}

pub fn initialize_kernel_context(
    stack_top: u64,
    entry: KernelContextEntry,
    arg: *mut (),
) -> KernelContext {
    let mut rsp = align_down(stack_top, 16);
    rsp -= core::mem::size_of::<KernelContextStartFrame>() as u64;
    let frame = rsp as *mut KernelContextStartFrame;
    unsafe {
        frame.write(KernelContextStartFrame { arg, entry });
    }

    KernelContext {
        rip: aether_x86_kernel_context_start as *const () as usize as u64,
        rsp,
        interrupts_enabled: 1,
        ..KernelContext::default()
    }
}

#[repr(C)]
struct TypedKernelContextEntry<T> {
    state: *mut T,
    entry: fn(&mut T) -> !,
}

unsafe extern "C" fn typed_kernel_context_entry<T>(arg: *mut ()) -> ! {
    let typed = unsafe { &*(arg.cast::<TypedKernelContextEntry<T>>()) };
    (typed.entry)(unsafe { &mut *typed.state });
}

pub fn initialize_typed_kernel_context<T>(
    stack_top: u64,
    state: &mut T,
    entry: fn(&mut T) -> !,
) -> KernelContext {
    let mut typed_entry_top = align_down(stack_top, 16);
    typed_entry_top -= core::mem::size_of::<TypedKernelContextEntry<T>>() as u64;
    let typed_entry_ptr = typed_entry_top as *mut TypedKernelContextEntry<T>;
    unsafe {
        typed_entry_ptr.write(TypedKernelContextEntry { state, entry });
    }
    initialize_kernel_context(
        typed_entry_top,
        typed_kernel_context_entry::<T>,
        typed_entry_top as *mut (),
    )
}

/// # Safety
///
/// `save` and `load` must point to valid kernel-context records whose stacks
/// remain alive for the duration of the context switch.
pub unsafe fn switch_kernel_context(save: *mut KernelContext, load: *const KernelContext) {
    unsafe { aether_x86_switch_kernel_context(save, load) }
}

pub fn resume_kernel_context(scheduler: &mut KernelContext, process: &KernelContext) {
    unsafe {
        switch_kernel_context(scheduler, process);
    }
}

pub fn install_scheduler_context(context: &mut KernelContext) {
    let _ = CURRENT_SCHEDULER_CONTEXTS.store(
        crate::arch::cpu::current_cpu_index(),
        context as *mut KernelContext,
    );
}

pub fn clear_scheduler_context() {
    let _ =
        CURRENT_SCHEDULER_CONTEXTS.store(crate::arch::cpu::current_cpu_index(), ptr::null_mut());
}

pub fn switch_to_scheduler(current: &mut KernelContext) {
    let scheduler = CURRENT_SCHEDULER_CONTEXTS
        .load(crate::arch::cpu::current_cpu_index())
        .unwrap_or(ptr::null_mut());
    assert!(
        !scheduler.is_null(),
        "scheduler context not installed for current cpu"
    );
    unsafe {
        switch_kernel_context(current, scheduler);
    }
}

const fn align_down(value: u64, align: u64) -> u64 {
    value & !(align - 1)
}

fn restore_user_tls_bases(context: &UserContext) {
    let mut fs_base = Msr::new(IA32_FS_BASE);
    let mut gs_base = Msr::new(IA32_GS_BASE);
    unsafe {
        fs_base.write(context.fs_base());
        gs_base.write(context.gs_base());
    }
}

fn resume_mode_for_context(context: &UserContext) -> ResumeMode {
    // The continued kernel-side syscall execution model still relies on iret
    // for architecturally conservative returns after a trapped syscall.
    let _ = context;
    ResumeMode::Iret
}

const fn sanitized_user_rflags(rflags: u64) -> u64 {
    const RFLAGS_FIXED: u64 = (1 << 1) | (1 << 9) | (1 << 21);
    rflags | RFLAGS_FIXED
}

unsafe extern "C" {
    fn aether_x86_enter_user_iret(context: *const UserContext, user_cr3: u64, current_run: *mut ());
    fn aether_x86_enter_user_sysret(
        context: *const UserContext,
        user_cr3: u64,
        current_run: *mut (),
    );
    fn aether_x86_call_on_stack(stack_top: u64, entry: usize, arg: usize) -> usize;
    fn aether_x86_switch_kernel_context(save: *mut KernelContext, load: *const KernelContext);
    fn aether_x86_kernel_context_start();
}

global_asm!(
    r#"
    .macro LOAD_USER_GPRS ctx
        mov r15, [\ctx + {general_off} + {r15_off}]
        mov r14, [\ctx + {general_off} + {r14_off}]
        mov r13, [\ctx + {general_off} + {r13_off}]
        mov r12, [\ctx + {general_off} + {r12_off}]
        mov r11, [\ctx + {general_off} + {r11_off}]
        mov r10, [\ctx + {general_off} + {r10_off}]
        mov r9, [\ctx + {general_off} + {r9_off}]
        mov r8, [\ctx + {general_off} + {r8_off}]
        mov rbp, [\ctx + {general_off} + {rbp_off}]
        mov rbx, [\ctx + {general_off} + {rbx_off}]
        mov rdx, [\ctx + {general_off} + {rdx_off}]
        mov rcx, [\ctx + {general_off} + {rcx_off}]
        mov rax, [\ctx + {general_off} + {rax_off}]
    .endm

    .global aether_x86_enter_user_iret
    aether_x86_enter_user_iret:
        mov [rdx + {run_saved_rbx_off}], rbx
        mov [rdx + {run_saved_rbp_off}], rbp
        mov [rdx + {run_saved_r12_off}], r12
        mov [rdx + {run_saved_r13_off}], r13
        mov [rdx + {run_saved_r14_off}], r14
        mov [rdx + {run_saved_r15_off}], r15
        mov [rdx + {run_kernel_rsp_off}], rsp
        pushfq
        pop rax
        shr rax, 9
        and eax, 1
        mov [rdx + {run_kernel_if_off}], rax
        mov cr3, rsi
        mov rsi, rdi

        push {user_ss}
        mov rax, [rsi + {general_off} + {rsp_off}]
        push rax
        mov rax, [rsi + {general_off} + {rflags_off}]
        or rax, {user_rflags_fixed}
        push rax
        push {user_cs}
        mov rax, [rsi + {general_off} + {rip_off}]
        push rax

        LOAD_USER_GPRS rsi
        mov rdi, [rsi + {general_off} + {rdi_off}]
        mov rbp, [rsi + {general_off} + {rbp_off}]
        mov rbx, [rsi + {general_off} + {rbx_off}]
        mov rdx, [rsi + {general_off} + {rdx_off}]
        mov rcx, [rsi + {general_off} + {rcx_off}]
        mov rax, [rsi + {general_off} + {rax_off}]
        mov rsi, [rsi + {general_off} + {rsi_off}]
        iretq

    .global aether_x86_enter_user_sysret
    aether_x86_enter_user_sysret:
        mov [rdx + {run_saved_rbx_off}], rbx
        mov [rdx + {run_saved_rbp_off}], rbp
        mov [rdx + {run_saved_r12_off}], r12
        mov [rdx + {run_saved_r13_off}], r13
        mov [rdx + {run_saved_r14_off}], r14
        mov [rdx + {run_saved_r15_off}], r15
        mov [rdx + {run_kernel_rsp_off}], rsp
        pushfq
        pop rax
        shr rax, 9
        and eax, 1
        mov [rdx + {run_kernel_if_off}], rax
        mov cr3, rsi
        mov rsi, rdi

        mov rsp, [rsi + {general_off} + {rsp_off}]
        mov rcx, [rsi + {general_off} + {rip_off}]
        mov r11, [rsi + {general_off} + {rflags_off}]
        or r11, {user_rflags_fixed}
        mov r15, [rsi + {general_off} + {r15_off}]
        mov r14, [rsi + {general_off} + {r14_off}]
        mov r13, [rsi + {general_off} + {r13_off}]
        mov r12, [rsi + {general_off} + {r12_off}]
        mov r10, [rsi + {general_off} + {r10_off}]
        mov r9, [rsi + {general_off} + {r9_off}]
        mov r8, [rsi + {general_off} + {r8_off}]
        mov rbp, [rsi + {general_off} + {rbp_off}]
        mov rbx, [rsi + {general_off} + {rbx_off}]
        mov rdx, [rsi + {general_off} + {rdx_off}]
        mov rax, [rsi + {general_off} + {rax_off}]
        mov rdi, [rsi + {general_off} + {rdi_off}]
        mov rsi, [rsi + {general_off} + {rsi_off}]
        sysretq

    .global aether_x86_call_on_stack
    aether_x86_call_on_stack:
        mov r8, rsp
        mov rsp, rdi
        and rsp, -16
        sub rsp, 16
        mov [rsp], r8
        mov rdi, rdx
        call rsi
        mov rdx, [rsp]
        mov rsp, rdx
        ret

    .global aether_x86_switch_kernel_context
    aether_x86_switch_kernel_context:
        lea rax, [rip + 1f]
        mov [rdi + {ctx_rip_off}], rax
        mov [rdi + {ctx_rsp_off}], rsp
        pushfq
        pop rax
        shr rax, 9
        and eax, 1
        mov [rdi + {ctx_if_off}], rax
        mov [rdi + {ctx_saved_rbx_off}], rbx
        mov [rdi + {ctx_saved_rbp_off}], rbp
        mov [rdi + {ctx_saved_r12_off}], r12
        mov [rdi + {ctx_saved_r13_off}], r13
        mov [rdi + {ctx_saved_r14_off}], r14
        mov [rdi + {ctx_saved_r15_off}], r15
        mov rax, [rsi + {ctx_rip_off}]
        mov rsp, [rsi + {ctx_rsp_off}]
        mov rbx, [rsi + {ctx_saved_rbx_off}]
        mov rbp, [rsi + {ctx_saved_rbp_off}]
        mov r12, [rsi + {ctx_saved_r12_off}]
        mov r13, [rsi + {ctx_saved_r13_off}]
        mov r14, [rsi + {ctx_saved_r14_off}]
        mov r15, [rsi + {ctx_saved_r15_off}]
        cmp qword ptr [rsi + {ctx_if_off}], 0
        je 2f
        sti
        jmp 3f
    2:
        cli
    3:
        jmp rax
    1:
        ret

    .global aether_x86_kernel_context_start
    aether_x86_kernel_context_start:
        pop rdi
        pop rax
        jmp rax
        ud2
    "#,
    run_kernel_rsp_off = const offset_of!(CurrentRun, kernel_rsp),
    run_saved_rbx_off = const offset_of!(CurrentRun, saved_rbx),
    run_saved_rbp_off = const offset_of!(CurrentRun, saved_rbp),
    run_saved_r12_off = const offset_of!(CurrentRun, saved_r12),
    run_saved_r13_off = const offset_of!(CurrentRun, saved_r13),
    run_saved_r14_off = const offset_of!(CurrentRun, saved_r14),
    run_saved_r15_off = const offset_of!(CurrentRun, saved_r15),
    run_kernel_if_off = const offset_of!(CurrentRun, kernel_interrupts_enabled),
    ctx_rip_off = const offset_of!(KernelContext, rip),
    ctx_rsp_off = const offset_of!(KernelContext, rsp),
    ctx_if_off = const offset_of!(KernelContext, interrupts_enabled),
    ctx_saved_rbx_off = const offset_of!(KernelContext, saved_rbx),
    ctx_saved_rbp_off = const offset_of!(KernelContext, saved_rbp),
    ctx_saved_r12_off = const offset_of!(KernelContext, saved_r12),
    ctx_saved_r13_off = const offset_of!(KernelContext, saved_r13),
    ctx_saved_r14_off = const offset_of!(KernelContext, saved_r14),
    ctx_saved_r15_off = const offset_of!(KernelContext, saved_r15),
    general_off = const offset_of!(UserContext, general),
    r15_off = const offset_of!(GeneralRegs, r15),
    r14_off = const offset_of!(GeneralRegs, r14),
    r13_off = const offset_of!(GeneralRegs, r13),
    r12_off = const offset_of!(GeneralRegs, r12),
    r11_off = const offset_of!(GeneralRegs, r11),
    r10_off = const offset_of!(GeneralRegs, r10),
    r9_off = const offset_of!(GeneralRegs, r9),
    r8_off = const offset_of!(GeneralRegs, r8),
    rdi_off = const offset_of!(GeneralRegs, rdi),
    rsi_off = const offset_of!(GeneralRegs, rsi),
    rbp_off = const offset_of!(GeneralRegs, rbp),
    rbx_off = const offset_of!(GeneralRegs, rbx),
    rdx_off = const offset_of!(GeneralRegs, rdx),
    rcx_off = const offset_of!(GeneralRegs, rcx),
    rax_off = const offset_of!(GeneralRegs, rax),
    rip_off = const offset_of!(GeneralRegs, rip),
    rsp_off = const offset_of!(GeneralRegs, rsp),
    rflags_off = const offset_of!(GeneralRegs, rflags),
    user_rflags_fixed = const sanitized_user_rflags(0),
    user_cs = const super::super::interrupt::gdt::USER_CODE_SELECTOR as u64,
    user_ss = const super::super::interrupt::gdt::USER_DATA_SELECTOR as u64,
);
