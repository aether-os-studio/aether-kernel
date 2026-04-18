use core::arch::global_asm;
use core::mem::offset_of;
use core::ptr;

use x86_64::registers::model_specific::Msr;

use crate::arch::process::{CurrentRun, GeneralRegs, UserContext};
use crate::boot::MAX_CPUS;
use crate::interrupt::Trap;
use crate::libs::percpu::PerCpu;

use super::gdt::{KERNEL_CODE_SELECTOR, USER_DATA_SELECTOR};

const IA32_EFER: u32 = 0xc000_0080;
const IA32_STAR: u32 = 0xc000_0081;
const IA32_LSTAR: u32 = 0xc000_0082;
const IA32_FMASK: u32 = 0xc000_0084;
const IA32_KERNEL_GS_BASE: u32 = 0xc000_0102;

const EFER_SCE: u64 = 1 << 0;
const RFLAGS_TF: u64 = 1 << 8;
const RFLAGS_IF: u64 = 1 << 9;
const RFLAGS_DF: u64 = 1 << 10;

const REG_SAVE_BYTES: usize = 15 * 8;
const FRAME_KIND_INTERRUPT: u64 = 0;
const FRAME_KIND_SYSCALL: u64 = 1;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TrapFrame {
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
    pub vector: u64,
    pub error_code: u64,
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
    pub kind: u64,
}

impl TrapFrame {
    #[must_use]
    pub const fn vector(&self) -> u8 {
        self.vector as u8
    }

    #[must_use]
    pub const fn error_code(&self) -> u64 {
        self.error_code
    }

    #[must_use]
    pub const fn rip(&self) -> u64 {
        self.rip
    }

    #[must_use]
    pub const fn from_user(&self) -> bool {
        (self.cs & 0x3) == 0x3
    }

    #[must_use]
    pub const fn is_syscall(&self) -> bool {
        self.kind == FRAME_KIND_SYSCALL
            || self.vector == crate::interrupt::SYSCALL_TRAP_VECTOR as u64
    }
}

#[repr(C)]
pub struct UserEntryState {
    pub kernel_rsp: u64,
    pub user_rsp: u64,
    pub current_run: u64,
    pub user_context: u64,
}

static USER_ENTRY_STATE: PerCpu<UserEntryState, MAX_CPUS> = PerCpu::uninit();

pub fn init_syscall(cpu_index: usize) -> Result<(), &'static str> {
    let mut efer = Msr::new(IA32_EFER);
    let mut star = Msr::new(IA32_STAR);
    let mut lstar = Msr::new(IA32_LSTAR);
    let mut fmask = Msr::new(IA32_FMASK);
    let mut kernel_gs_base = Msr::new(IA32_KERNEL_GS_BASE);
    USER_ENTRY_STATE
        .init(
            cpu_index,
            UserEntryState {
                kernel_rsp: 0,
                user_rsp: 0,
                current_run: 0,
                user_context: 0,
            },
        )
        .map_err(|_| "failed to initialize per-cpu user entry state")?;
    let gs_base = USER_ENTRY_STATE
        .with(cpu_index, |data| core::ptr::from_ref(data) as u64)
        .map_err(|_| "per-cpu user entry state is unavailable")?;

    unsafe {
        efer.write(efer.read() | EFER_SCE);
        star.write(make_star_value());
        lstar.write(aether_x86_syscall_entry as *const () as usize as u64);
        fmask.write(RFLAGS_IF | RFLAGS_TF | RFLAGS_DF);
        kernel_gs_base.write(gs_base);
    }

    Ok(())
}

pub fn set_syscall_kernel_stack(stack_top: u64) {
    let _ = USER_ENTRY_STATE.with_mut(crate::arch::cpu::current_cpu_index(), |data| {
        data.kernel_rsp = stack_top;
    });
}

pub fn set_user_entry_context(current_run: *const CurrentRun, user_context: *mut UserContext) {
    let _ = USER_ENTRY_STATE.with_mut(crate::arch::cpu::current_cpu_index(), |data| {
        data.current_run = current_run as u64;
        data.user_context = user_context as u64;
    });
}

const fn make_star_value() -> u64 {
    let kernel_cs = KERNEL_CODE_SELECTOR as u64;
    let user_sysret_base = (USER_DATA_SELECTOR - 8) as u64;
    (user_sysret_base << 48) | (kernel_cs << 32)
}

#[unsafe(no_mangle)]
extern "C" fn aether_x86_dispatch_kernel_trap(frame: &mut TrapFrame) -> *const CurrentRun {
    let trap = Trap::from_frame(frame);

    crate::arch::fpu::save_kernel_interrupt_state();
    crate::interrupt::dispatch_trap(trap, frame);
    if matches!(trap.kind(), crate::interrupt::TrapKind::Interrupt) {
        super::finish_interrupt(trap.vector());
        crate::interrupt::softirq::drain_pending();
    }
    crate::arch::fpu::restore_kernel_interrupt_state();
    ptr::null()
}

unsafe extern "C" {
    fn aether_x86_syscall_entry();
}

global_asm!(
    r#"
    .altmacro
    .macro AETHER_PUSH_REGS
        push rax
        push rcx
        push rdx
        push rbx
        push rbp
        push rsi
        push rdi
        push r8
        push r9
        push r10
        push r11
        push r12
        push r13
        push r14
        push r15
    .endm

    .macro AETHER_POP_REGS
        pop r15
        pop r14
        pop r13
        pop r12
        pop r11
        pop r10
        pop r9
        pop r8
        pop rdi
        pop rsi
        pop rbp
        pop rbx
        pop rdx
        pop rcx
        pop rax
    .endm

    .macro AETHER_WRITE_FRAME dst, src
        mov rax, [\src + 0]
        mov [\dst + {r15_off}], rax
        mov rax, [\src + 8]
        mov [\dst + {r14_off}], rax
        mov rax, [\src + 16]
        mov [\dst + {r13_off}], rax
        mov rax, [\src + 24]
        mov [\dst + {r12_off}], rax
        mov rax, [\src + 32]
        mov [\dst + {r11_off}], rax
        mov rax, [\src + 40]
        mov [\dst + {r10_off}], rax
        mov rax, [\src + 48]
        mov [\dst + {r9_off}], rax
        mov rax, [\src + 56]
        mov [\dst + {r8_off}], rax
        mov rax, [\src + 64]
        mov [\dst + {rdi_off}], rax
        mov rax, [\src + 72]
        mov [\dst + {rsi_off}], rax
        mov rax, [\src + 80]
        mov [\dst + {rbp_off}], rax
        mov rax, [\src + 88]
        mov [\dst + {rbx_off}], rax
        mov rax, [\src + 96]
        mov [\dst + {rdx_off}], rax
        mov rax, [\src + 104]
        mov [\dst + {rcx_off}], rax
        mov rax, [\src + 112]
        mov [\dst + {rax_off}], rax
    .endm

    .macro AETHER_WRITE_USER_CONTEXT dst, src
        mov rax, [\src + 0]
        mov [\dst + {ctx_r15_off}], rax
        mov rax, [\src + 8]
        mov [\dst + {ctx_r14_off}], rax
        mov rax, [\src + 16]
        mov [\dst + {ctx_r13_off}], rax
        mov rax, [\src + 24]
        mov [\dst + {ctx_r12_off}], rax
        mov rax, [\src + 32]
        mov [\dst + {ctx_r11_off}], rax
        mov rax, [\src + 40]
        mov [\dst + {ctx_r10_off}], rax
        mov rax, [\src + 48]
        mov [\dst + {ctx_r9_off}], rax
        mov rax, [\src + 56]
        mov [\dst + {ctx_r8_off}], rax
        mov rax, [\src + 64]
        mov [\dst + {ctx_rdi_off}], rax
        mov rax, [\src + 72]
        mov [\dst + {ctx_rsi_off}], rax
        mov rax, [\src + 80]
        mov [\dst + {ctx_rbp_off}], rax
        mov rax, [\src + 88]
        mov [\dst + {ctx_rbx_off}], rax
        mov rax, [\src + 96]
        mov [\dst + {ctx_rdx_off}], rax
        mov rax, [\src + 104]
        mov [\dst + {ctx_rcx_off}], rax
        mov rax, [\src + 112]
        mov [\dst + {ctx_rax_off}], rax
        mov rax, [\src + {reg_bytes}]
        mov [\dst + {ctx_trap_num_off}], rax
        mov rax, [\src + {reg_bytes} + 8]
        mov [\dst + {ctx_error_off}], rax
        mov rax, [\src + {reg_bytes} + 16]
        mov [\dst + {ctx_rip_off}], rax
        mov rax, [\src + {reg_bytes} + 32]
        mov [\dst + {ctx_rflags_off}], rax
        mov rax, [\src + {reg_bytes} + 40]
        mov [\dst + {ctx_rsp_off}], rax
    .endm

    .macro AETHER_BUILD_INTERRUPT_FRAME
        lea rsi, [rsp + {frame_size}]
        AETHER_WRITE_FRAME rsp, rsi

        mov rax, [rsi + {reg_bytes}]
        mov [rsp + {vector_off}], rax
        mov rax, [rsi + {reg_bytes} + 8]
        mov [rsp + {error_off}], rax
        mov rax, [rsi + {reg_bytes} + 16]
        mov [rsp + {rip_off}], rax
        mov rax, [rsi + {reg_bytes} + 24]
        mov [rsp + {cs_off}], rax
        mov rax, [rsi + {reg_bytes} + 32]
        mov [rsp + {rflags_off}], rax

        test byte ptr [rsi + {reg_bytes} + 24], 3
        jz 2f
        mov rax, [rsi + {reg_bytes} + 40]
        mov [rsp + {rsp_off}], rax
        mov rax, [rsi + {reg_bytes} + 48]
        mov [rsp + {ss_off}], rax
        jmp 3f
    2:
        lea rax, [rsi + {reg_bytes} + 40]
        mov [rsp + {rsp_off}], rax
        xor eax, eax
        mov ax, ss
        mov [rsp + {ss_off}], rax
    3:
        mov qword ptr [rsp + {kind_off}], {interrupt_kind}
    .endm

    .macro DECLARE_INTERRUPT_STUB vector
    .global aether_x86_trap_stub_\vector
    aether_x86_trap_stub_\vector:
        push 0
        push \vector
        jmp aether_x86_interrupt_entry
    .endm

    .macro DECLARE_ERROR_STUB vector
    .global aether_x86_trap_stub_\vector
    aether_x86_trap_stub_\vector:
        push \vector
        jmp aether_x86_interrupt_entry
    .endm

    .global aether_x86_idt_stub_table
    aether_x86_idt_stub_table:
        .quad aether_x86_trap_stub_0
        .quad aether_x86_trap_stub_1
        .quad aether_x86_trap_stub_2
        .quad aether_x86_trap_stub_3
        .quad aether_x86_trap_stub_4
        .quad aether_x86_trap_stub_5
        .quad aether_x86_trap_stub_6
        .quad aether_x86_trap_stub_7
        .quad aether_x86_trap_stub_8
        .quad aether_x86_trap_stub_9
        .quad aether_x86_trap_stub_10
        .quad aether_x86_trap_stub_11
        .quad aether_x86_trap_stub_12
        .quad aether_x86_trap_stub_13
        .quad aether_x86_trap_stub_14
        .quad aether_x86_trap_stub_15
        .quad aether_x86_trap_stub_16
        .quad aether_x86_trap_stub_17
        .quad aether_x86_trap_stub_18
        .quad aether_x86_trap_stub_19
        .quad aether_x86_trap_stub_20
        .quad aether_x86_trap_stub_21
        .quad aether_x86_trap_stub_22
        .quad aether_x86_trap_stub_23
        .quad aether_x86_trap_stub_24
        .quad aether_x86_trap_stub_25
        .quad aether_x86_trap_stub_26
        .quad aether_x86_trap_stub_27
        .quad aether_x86_trap_stub_28
        .quad aether_x86_trap_stub_29
        .quad aether_x86_trap_stub_30
        .quad aether_x86_trap_stub_31
        .irp vec, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 75, 76, 77, 78, 79, 80, 81, 82, 83, 84, 85, 86, 87, 88, 89, 90, 91, 92, 93, 94, 95, 96, 97, 98, 99, 100, 101, 102, 103, 104, 105, 106, 107, 108, 109, 110, 111, 112, 113, 114, 115, 116, 117, 118, 119, 120, 121, 122, 123, 124, 125, 126, 127, 128, 129, 130, 131, 132, 133, 134, 135, 136, 137, 138, 139, 140, 141, 142, 143, 144, 145, 146, 147, 148, 149, 150, 151, 152, 153, 154, 155, 156, 157, 158, 159, 160, 161, 162, 163, 164, 165, 166, 167, 168, 169, 170, 171, 172, 173, 174, 175, 176, 177, 178, 179, 180, 181, 182, 183, 184, 185, 186, 187, 188, 189, 190, 191, 192, 193, 194, 195, 196, 197, 198, 199, 200, 201, 202, 203, 204, 205, 206, 207, 208, 209, 210, 211, 212, 213, 214, 215, 216, 217, 218, 219, 220, 221, 222, 223, 224, 225, 226, 227, 228, 229, 230, 231, 232, 233, 234, 235, 236, 237, 238, 239, 240, 241, 242, 243, 244, 245, 246, 247, 248, 249, 250, 251, 252, 253, 254, 255
            .quad aether_x86_trap_stub_\vec
        .endr

    DECLARE_INTERRUPT_STUB 0
    DECLARE_INTERRUPT_STUB 1
    DECLARE_INTERRUPT_STUB 2
    DECLARE_INTERRUPT_STUB 3
    DECLARE_INTERRUPT_STUB 4
    DECLARE_INTERRUPT_STUB 5
    DECLARE_INTERRUPT_STUB 6
    DECLARE_INTERRUPT_STUB 7
    DECLARE_ERROR_STUB 8
    DECLARE_INTERRUPT_STUB 9
    DECLARE_ERROR_STUB 10
    DECLARE_ERROR_STUB 11
    DECLARE_ERROR_STUB 12
    DECLARE_ERROR_STUB 13
    DECLARE_ERROR_STUB 14
    DECLARE_INTERRUPT_STUB 15
    DECLARE_INTERRUPT_STUB 16
    DECLARE_ERROR_STUB 17
    DECLARE_INTERRUPT_STUB 18
    DECLARE_INTERRUPT_STUB 19
    DECLARE_INTERRUPT_STUB 20
    DECLARE_ERROR_STUB 21
    DECLARE_INTERRUPT_STUB 22
    DECLARE_INTERRUPT_STUB 23
    DECLARE_INTERRUPT_STUB 24
    DECLARE_INTERRUPT_STUB 25
    DECLARE_INTERRUPT_STUB 26
    DECLARE_INTERRUPT_STUB 27
    DECLARE_INTERRUPT_STUB 28
    DECLARE_ERROR_STUB 29
    DECLARE_ERROR_STUB 30
    DECLARE_INTERRUPT_STUB 31
    .irp vec, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 75, 76, 77, 78, 79, 80, 81, 82, 83, 84, 85, 86, 87, 88, 89, 90, 91, 92, 93, 94, 95, 96, 97, 98, 99, 100, 101, 102, 103, 104, 105, 106, 107, 108, 109, 110, 111, 112, 113, 114, 115, 116, 117, 118, 119, 120, 121, 122, 123, 124, 125, 126, 127, 128, 129, 130, 131, 132, 133, 134, 135, 136, 137, 138, 139, 140, 141, 142, 143, 144, 145, 146, 147, 148, 149, 150, 151, 152, 153, 154, 155, 156, 157, 158, 159, 160, 161, 162, 163, 164, 165, 166, 167, 168, 169, 170, 171, 172, 173, 174, 175, 176, 177, 178, 179, 180, 181, 182, 183, 184, 185, 186, 187, 188, 189, 190, 191, 192, 193, 194, 195, 196, 197, 198, 199, 200, 201, 202, 203, 204, 205, 206, 207, 208, 209, 210, 211, 212, 213, 214, 215, 216, 217, 218, 219, 220, 221, 222, 223, 224, 225, 226, 227, 228, 229, 230, 231, 232, 233, 234, 235, 236, 237, 238, 239, 240, 241, 242, 243, 244, 245, 246, 247, 248, 249, 250, 251, 252, 253, 254, 255
        DECLARE_INTERRUPT_STUB \vec
    .endr

    aether_x86_interrupt_entry:
        cld
        AETHER_PUSH_REGS
        test byte ptr [rsp + {reg_bytes} + 24], 3
        jz 1f
        swapgs
        mov rdi, gs:[{entry_user_ctx_off}]
        AETHER_WRITE_USER_CONTEXT rdi, rsp
        mov rax, gs:[{entry_current_run_off}]
        swapgs
        jmp aether_x86_trap_exit_to_kernel
    1:
        sub rsp, {frame_size}
        AETHER_BUILD_INTERRUPT_FRAME
        mov rdi, rsp
        call {dispatch_kernel}
        test rax, rax
        jnz aether_x86_trap_exit_to_kernel
        add rsp, {frame_size}
        AETHER_POP_REGS
        add rsp, 16
        iretq

    .global aether_x86_syscall_entry
    aether_x86_syscall_entry:
        cld
        swapgs
        mov gs:[{entry_user_rsp_off}], rsp
        mov rsp, gs:[{entry_kernel_rsp_off}]
        push rdi
        mov rdi, gs:[{entry_user_ctx_off}]

        mov [rdi + {ctx_r15_off}], r15
        mov [rdi + {ctx_r14_off}], r14
        mov [rdi + {ctx_r13_off}], r13
        mov [rdi + {ctx_r12_off}], r12
        mov [rdi + {ctx_r11_off}], r11
        mov [rdi + {ctx_r10_off}], r10
        mov [rdi + {ctx_r9_off}], r9
        mov [rdi + {ctx_r8_off}], r8
        mov [rdi + {ctx_rax_off}], rax
        mov rax, [rsp]
        mov [rdi + {ctx_rdi_off}], rax
        mov [rdi + {ctx_rsi_off}], rsi
        mov [rdi + {ctx_rbp_off}], rbp
        mov [rdi + {ctx_rbx_off}], rbx
        mov [rdi + {ctx_rdx_off}], rdx
        mov [rdi + {ctx_rcx_off}], rcx
        mov qword ptr [rdi + {ctx_trap_num_off}], {syscall_vector}
        mov qword ptr [rdi + {ctx_error_off}], 0
        mov [rdi + {ctx_rip_off}], rcx
        mov [rdi + {ctx_rflags_off}], r11
        mov rax, gs:[{entry_user_rsp_off}]
        mov [rdi + {ctx_rsp_off}], rax

        mov rax, gs:[{entry_current_run_off}]
        add rsp, 8
        jmp aether_x86_syscall_exit_to_kernel

    aether_x86_syscall_exit_to_kernel:
        mov rdx, rax
        mov rcx, [rdx + {run_kernel_cr3_off}]
        mov cr3, rcx
        swapgs
        mov rsp, [rdx + {run_kernel_rsp_off}]
        mov rbx, [rdx + {run_saved_rbx_off}]
        mov rbp, [rdx + {run_saved_rbp_off}]
        mov r12, [rdx + {run_saved_r12_off}]
        mov r13, [rdx + {run_saved_r13_off}]
        mov r14, [rdx + {run_saved_r14_off}]
        mov r15, [rdx + {run_saved_r15_off}]
        cmp qword ptr [rdx + {run_kernel_if_off}], 0
        je 1f
        sti
        jmp 2f
    1:
        cli
    2:
        ret

    aether_x86_trap_exit_to_kernel:
        mov rdx, rax
        mov rcx, [rdx + {run_kernel_cr3_off}]
        mov cr3, rcx
        mov rsp, [rdx + {run_kernel_rsp_off}]
        mov rbx, [rdx + {run_saved_rbx_off}]
        mov rbp, [rdx + {run_saved_rbp_off}]
        mov r12, [rdx + {run_saved_r12_off}]
        mov r13, [rdx + {run_saved_r13_off}]
        mov r14, [rdx + {run_saved_r14_off}]
        mov r15, [rdx + {run_saved_r15_off}]
        cmp qword ptr [rdx + {run_kernel_if_off}], 0
        je 3f
        sti
        jmp 4f
    3:
        cli
    4:
        ret
    "#,
    frame_size = const core::mem::size_of::<TrapFrame>(),
    reg_bytes = const REG_SAVE_BYTES,
    r15_off = const offset_of!(TrapFrame, r15),
    r14_off = const offset_of!(TrapFrame, r14),
    r13_off = const offset_of!(TrapFrame, r13),
    r12_off = const offset_of!(TrapFrame, r12),
    r11_off = const offset_of!(TrapFrame, r11),
    r10_off = const offset_of!(TrapFrame, r10),
    r9_off = const offset_of!(TrapFrame, r9),
    r8_off = const offset_of!(TrapFrame, r8),
    rdi_off = const offset_of!(TrapFrame, rdi),
    rsi_off = const offset_of!(TrapFrame, rsi),
    rbp_off = const offset_of!(TrapFrame, rbp),
    rbx_off = const offset_of!(TrapFrame, rbx),
    rdx_off = const offset_of!(TrapFrame, rdx),
    rcx_off = const offset_of!(TrapFrame, rcx),
    rax_off = const offset_of!(TrapFrame, rax),
    vector_off = const offset_of!(TrapFrame, vector),
    error_off = const offset_of!(TrapFrame, error_code),
    rip_off = const offset_of!(TrapFrame, rip),
    cs_off = const offset_of!(TrapFrame, cs),
    rflags_off = const offset_of!(TrapFrame, rflags),
    rsp_off = const offset_of!(TrapFrame, rsp),
    ss_off = const offset_of!(TrapFrame, ss),
    kind_off = const offset_of!(TrapFrame, kind),
    syscall_vector = const crate::interrupt::SYSCALL_TRAP_VECTOR as u64,
    interrupt_kind = const FRAME_KIND_INTERRUPT,
    dispatch_kernel = sym aether_x86_dispatch_kernel_trap,
    run_kernel_rsp_off = const offset_of!(CurrentRun, kernel_rsp),
    run_kernel_cr3_off = const offset_of!(CurrentRun, kernel_cr3),
    run_saved_rbx_off = const offset_of!(CurrentRun, saved_rbx),
    run_saved_rbp_off = const offset_of!(CurrentRun, saved_rbp),
    run_saved_r12_off = const offset_of!(CurrentRun, saved_r12),
    run_saved_r13_off = const offset_of!(CurrentRun, saved_r13),
    run_saved_r14_off = const offset_of!(CurrentRun, saved_r14),
    run_saved_r15_off = const offset_of!(CurrentRun, saved_r15),
    run_kernel_if_off = const offset_of!(CurrentRun, kernel_interrupts_enabled),
    ctx_r15_off = const offset_of!(UserContext, general) + offset_of!(GeneralRegs, r15),
    ctx_r14_off = const offset_of!(UserContext, general) + offset_of!(GeneralRegs, r14),
    ctx_r13_off = const offset_of!(UserContext, general) + offset_of!(GeneralRegs, r13),
    ctx_r12_off = const offset_of!(UserContext, general) + offset_of!(GeneralRegs, r12),
    ctx_r11_off = const offset_of!(UserContext, general) + offset_of!(GeneralRegs, r11),
    ctx_r10_off = const offset_of!(UserContext, general) + offset_of!(GeneralRegs, r10),
    ctx_r9_off = const offset_of!(UserContext, general) + offset_of!(GeneralRegs, r9),
    ctx_r8_off = const offset_of!(UserContext, general) + offset_of!(GeneralRegs, r8),
    ctx_rdi_off = const offset_of!(UserContext, general) + offset_of!(GeneralRegs, rdi),
    ctx_rsi_off = const offset_of!(UserContext, general) + offset_of!(GeneralRegs, rsi),
    ctx_rbp_off = const offset_of!(UserContext, general) + offset_of!(GeneralRegs, rbp),
    ctx_rbx_off = const offset_of!(UserContext, general) + offset_of!(GeneralRegs, rbx),
    ctx_rdx_off = const offset_of!(UserContext, general) + offset_of!(GeneralRegs, rdx),
    ctx_rcx_off = const offset_of!(UserContext, general) + offset_of!(GeneralRegs, rcx),
    ctx_rax_off = const offset_of!(UserContext, general) + offset_of!(GeneralRegs, rax),
    ctx_rip_off = const offset_of!(UserContext, general) + offset_of!(GeneralRegs, rip),
    ctx_rsp_off = const offset_of!(UserContext, general) + offset_of!(GeneralRegs, rsp),
    ctx_rflags_off = const offset_of!(UserContext, general) + offset_of!(GeneralRegs, rflags),
    ctx_trap_num_off = const offset_of!(UserContext, trap_num),
    ctx_error_off = const offset_of!(UserContext, error_code),
    entry_kernel_rsp_off =
        const offset_of!(UserEntryState, kernel_rsp),
    entry_user_rsp_off =
        const offset_of!(UserEntryState, user_rsp),
    entry_current_run_off =
        const offset_of!(UserEntryState, current_run),
    entry_user_ctx_off =
        const offset_of!(UserEntryState, user_context),
);
