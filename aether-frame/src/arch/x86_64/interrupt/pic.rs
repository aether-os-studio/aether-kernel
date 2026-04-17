use crate::io::Port;

const PIC1_DATA_PORT: u16 = 0x21;
const PIC2_DATA_PORT: u16 = 0xa1;
const PIC1_COMMAND_PORT: u16 = 0x20;
const PIC2_COMMAND_PORT: u16 = 0xa0;
const PIC_EOI: u8 = 0x20;
const IMCR_SELECT_PORT: u16 = 0x22;
const IMCR_DATA_PORT: u16 = 0x23;
const IMCR_REGISTER: u8 = 0x70;
const IMCR_APIC_MODE: u8 = 0x01;

pub fn disable_legacy_pic() {
    unsafe { Port::<u8>::new(PIC1_DATA_PORT) }.write(0xff);
    unsafe { Port::<u8>::new(PIC2_DATA_PORT) }.write(0xff);

    unsafe { Port::<u8>::new(PIC1_COMMAND_PORT) }.write(PIC_EOI);
    unsafe { Port::<u8>::new(PIC2_COMMAND_PORT) }.write(PIC_EOI);

    unsafe { Port::<u8>::new(IMCR_SELECT_PORT) }.write(IMCR_REGISTER);
    unsafe { Port::<u8>::new(IMCR_DATA_PORT) }.write(IMCR_APIC_MODE);
}
