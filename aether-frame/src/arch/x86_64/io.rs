use core::arch::asm;

#[inline]
/// Writes one byte to an x86 legacy I/O port.
///
/// # Safety
/// The caller must ensure `port` is owned by the current driver and that
/// writing `value` to it is valid for the attached device.
pub unsafe fn outb(port: u16, value: u8) {
    unsafe {
        asm!(
            "out dx, al",
            in("dx") port,
            in("al") value,
            options(nomem, nostack, preserves_flags),
        );
    }
}

#[inline]
#[must_use]
/// Reads one byte from an x86 legacy I/O port.
///
/// # Safety
/// The caller must ensure `port` is readable on the current machine and that
/// the device protocol allows an 8-bit read at this time.
pub unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    unsafe {
        asm!(
            "in al, dx",
            in("dx") port,
            out("al") value,
            options(nomem, nostack, preserves_flags),
        );
    }
    value
}

#[inline]
/// Writes one 16-bit word to an x86 legacy I/O port.
///
/// # Safety
/// The caller must ensure `port` is owned by the current driver and accepts a
/// 16-bit write.
pub unsafe fn outw(port: u16, value: u16) {
    unsafe {
        asm!(
            "out dx, ax",
            in("dx") port,
            in("ax") value,
            options(nomem, nostack, preserves_flags),
        );
    }
}

#[inline]
#[must_use]
/// Reads one 16-bit word from an x86 legacy I/O port.
///
/// # Safety
/// The caller must ensure `port` is readable on the current machine and that
/// the device protocol allows a 16-bit read at this time.
pub unsafe fn inw(port: u16) -> u16 {
    let value: u16;
    unsafe {
        asm!(
            "in ax, dx",
            in("dx") port,
            out("ax") value,
            options(nomem, nostack, preserves_flags),
        );
    }
    value
}

#[inline]
/// Writes one 32-bit dword to an x86 legacy I/O port.
///
/// # Safety
/// The caller must ensure `port` is owned by the current driver and accepts a
/// 32-bit write.
pub unsafe fn outl(port: u16, value: u32) {
    unsafe {
        asm!(
            "out dx, eax",
            in("dx") port,
            in("eax") value,
            options(nomem, nostack, preserves_flags),
        );
    }
}

#[inline]
#[must_use]
/// Reads one 32-bit dword from an x86 legacy I/O port.
///
/// # Safety
/// The caller must ensure `port` is readable on the current machine and that
/// the device protocol allows a 32-bit read at this time.
pub unsafe fn inl(port: u16) -> u32 {
    let value: u32;
    unsafe {
        asm!(
            "in eax, dx",
            in("dx") port,
            out("eax") value,
            options(nomem, nostack, preserves_flags),
        );
    }
    value
}
