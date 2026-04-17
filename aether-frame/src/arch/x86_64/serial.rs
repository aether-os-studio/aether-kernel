use core::fmt::{self, Write};

use lazy_static::lazy_static;

use crate::libs::spin::SpinLock;

use super::io;

const COM1_BASE: u16 = 0x3f8;
const DATA: u16 = 0;
const INTERRUPT_ENABLE: u16 = 1;
const FIFO_CONTROL: u16 = 2;
const LINE_CONTROL: u16 = 3;
const MODEM_CONTROL: u16 = 4;
const LINE_STATUS: u16 = 5;

const LINE_STATUS_TX_EMPTY: u8 = 1 << 5;
const LINE_CONTROL_DLAB: u8 = 1 << 7;
const LINE_CONTROL_8N1: u8 = 0x03;
const FIFO_ENABLE_CLEAR_14B: u8 = 0xC7;
const MODEM_CONTROL_DTR_RTS_OUT2: u8 = 0x0B;

lazy_static! {
    static ref SERIAL1: SpinLock<Uart16550> = {
        let mut uart = Uart16550::new(COM1_BASE);
        uart.init();
        SpinLock::new(uart)
    };
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    let _ = SERIAL1.lock_irqsave().write_fmt(args);
}

#[macro_export]
macro_rules! serial_print {
    ($($arg:tt)*) => (
        $crate::arch::serial::_print(format_args!($($arg)*))
    );
}

#[macro_export]
macro_rules! serial_println {
    () => ($crate::serial_print!("\n"));
    ($($arg:tt)*) => ($crate::serial_print!("{}\n", format_args!($($arg)*)));
}

struct Uart16550 {
    base: u16,
    initialized: bool,
}

impl Uart16550 {
    const fn new(base: u16) -> Self {
        Self {
            base,
            initialized: false,
        }
    }

    fn init(&mut self) {
        if self.initialized {
            return;
        }

        unsafe {
            io::outb(self.base + INTERRUPT_ENABLE, 0x00);
            io::outb(self.base + LINE_CONTROL, LINE_CONTROL_DLAB);
            io::outb(self.base + DATA, 0x03);
            io::outb(self.base + INTERRUPT_ENABLE, 0x00);
            io::outb(self.base + LINE_CONTROL, LINE_CONTROL_8N1);
            io::outb(self.base + FIFO_CONTROL, FIFO_ENABLE_CLEAR_14B);
            io::outb(self.base + MODEM_CONTROL, MODEM_CONTROL_DTR_RTS_OUT2);
        }

        self.initialized = true;
    }

    fn write_byte(&mut self, byte: u8) {
        if !self.initialized {
            self.init();
        }

        if byte == b'\n' {
            self.write_raw(b'\r');
        }
        self.write_raw(byte);
    }

    fn write_raw(&self, byte: u8) {
        while unsafe { io::inb(self.base + LINE_STATUS) } & LINE_STATUS_TX_EMPTY == 0 {}
        unsafe {
            io::outb(self.base + DATA, byte);
        }
    }
}

impl Write for Uart16550 {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            self.write_byte(byte);
        }
        Ok(())
    }
}
