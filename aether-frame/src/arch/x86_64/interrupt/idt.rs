use core::arch::asm;
use core::mem::size_of;

use super::gdt::KERNEL_CODE_SELECTOR;

const INTERRUPT_GATE_FLAGS: u16 = 0x8e00;

#[repr(C, packed)]
struct DescriptorTablePointer {
    limit: u16,
    base: u64,
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct IdtEntry {
    offset_low: u16,
    selector: u16,
    options: u16,
    offset_mid: u16,
    offset_high: u32,
    reserved: u32,
}

impl IdtEntry {
    const fn missing() -> Self {
        Self {
            offset_low: 0,
            selector: 0,
            options: 0,
            offset_mid: 0,
            offset_high: 0,
            reserved: 0,
        }
    }

    const fn set_handler(&mut self, handler: u64) {
        self.offset_low = handler as u16;
        self.selector = KERNEL_CODE_SELECTOR;
        self.options = INTERRUPT_GATE_FLAGS;
        self.offset_mid = (handler >> 16) as u16;
        self.offset_high = (handler >> 32) as u32;
        self.reserved = 0;
    }
}

unsafe extern "C" {
    static aether_x86_idt_stub_table: [u64; 256];
}

static mut IDT: [IdtEntry; 256] = [IdtEntry::missing(); 256];

pub fn init() {
    unsafe {
        let idt = (&raw mut IDT).cast::<IdtEntry>();
        let handlers = &aether_x86_idt_stub_table;
        for (vector, handler) in handlers.iter().copied().enumerate() {
            (*idt.add(vector)).set_handler(handler);
        }

        let idtr = DescriptorTablePointer {
            limit: (size_of::<[IdtEntry; 256]>() - 1) as u16,
            base: (&raw const IDT).cast::<()>() as u64,
        };

        asm!("lidt [{}]", in(reg) &raw const idtr, options(readonly, nostack, preserves_flags));
    }
}
