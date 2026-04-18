#![no_std]

extern crate alloc;

use aether_drivers::input::{
    EV_KEY, InputDevice, InputEventSink, KEY_0, KEY_1, KEY_2, KEY_3, KEY_4, KEY_5, KEY_6, KEY_7,
    KEY_8, KEY_9, KEY_A, KEY_APOSTROPHE, KEY_B, KEY_BACKSLASH, KEY_BACKSPACE, KEY_C, KEY_CAPSLOCK,
    KEY_COMMA, KEY_D, KEY_DELETE, KEY_DOT, KEY_DOWN, KEY_E, KEY_END, KEY_ENTER, KEY_EQUAL, KEY_ESC,
    KEY_F, KEY_G, KEY_GRAVE, KEY_H, KEY_HOME, KEY_I, KEY_INSERT, KEY_J, KEY_K, KEY_KP0, KEY_KP1,
    KEY_KP2, KEY_KP3, KEY_KP4, KEY_KP5, KEY_KP6, KEY_KP7, KEY_KP8, KEY_KP9, KEY_KPASTERISK,
    KEY_KPDOT, KEY_KPENTER, KEY_KPEQUAL, KEY_KPMINUS, KEY_KPPLUS, KEY_KPSLASH, KEY_L, KEY_LEFT,
    KEY_LEFTALT, KEY_LEFTBRACE, KEY_LEFTCTRL, KEY_LEFTSHIFT, KEY_M, KEY_MINUS, KEY_N, KEY_O, KEY_P,
    KEY_PAGEDOWN, KEY_PAGEUP, KEY_Q, KEY_R, KEY_RIGHT, KEY_RIGHTALT, KEY_RIGHTBRACE, KEY_RIGHTCTRL,
    KEY_RIGHTSHIFT, KEY_S, KEY_SEMICOLON, KEY_SLASH, KEY_SPACE, KEY_T, KEY_TAB, KEY_U, KEY_UP,
    KEY_V, KEY_W, KEY_X, KEY_Y, KEY_Z, LinuxInputEvent,
};
use aether_frame::serial_print;
use alloc::boxed::Box;
use alloc::sync::Arc;
use core::fmt;
use core::mem::size_of;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use aether_device::{DeviceClass, DeviceMetadata, DeviceNode, KernelDevice, default_console_alias};
use aether_frame::libs::spin::{LocalIrqDisabled, SpinLock};
use aether_framebuffer::{FramebufferSurface, RgbColor};
use aether_vfs::{
    FileNode, FileOperations, FsError, FsResult, IoctlResponse, PollEvents, SharedWaitListener,
    WaitQueue,
};
use os_terminal::font::BitmapFont;
use os_terminal::{DrawTarget, Rgb, Terminal};
use spin::Once;

const NCCS: usize = 19;
const TIOCGWINSZ: u64 = 0x5413;
const TCGETS: u64 = 0x5401;
const TCGETS2: u64 = 0x802c_542a;
const TIOCGPGRP: u64 = 0x540f;
const KDGETMODE: u64 = 0x4b3b;
const KDGKBMODE: u64 = 0x4b44;
const VT_GETMODE: u64 = 0x5601;
const VT_GETSTATE: u64 = 0x5603;
const VT_OPENQRY: u64 = 0x5600;
const KD_TEXT: i32 = 0x00;
const KD_GRAPHICS: i32 = 0x01;

const VINTR: usize = 0;
const VQUIT: usize = 1;
const VERASE: usize = 2;
const VKILL: usize = 3;
const VEOF: usize = 4;
const VTIME: usize = 5;
const VMIN: usize = 6;
const VSTART: usize = 8;
const VSTOP: usize = 9;
const VSUSP: usize = 10;
const VEOL: usize = 11;

const BRKINT: u32 = 0o000002;
const IGNCR: u32 = 0o000200;
const INLCR: u32 = 0o000100;
const INPCK: u32 = 0o000020;
const ISTRIP: u32 = 0o000040;
const ICRNL: u32 = 0o000400;
const IXON: u32 = 0o002000;
const OPOST: u32 = 0o000001;
const ONLCR: u32 = 0o000004;
const B38400: u32 = 0o000017;
const CS8: u32 = 0o000060;
const CREAD: u32 = 0o000200;
const HUPCL: u32 = 0o002000;
const ISIG: u32 = 0o000001;
const ICANON: u32 = 0o000002;
const ECHO: u32 = 0o000010;
const ECHOE: u32 = 0o000020;
const ECHOK: u32 = 0o000040;

const SIGINT: i32 = 2;
const SIGQUIT: i32 = 3;
const SIGTSTP: i32 = 20;
const TTY_INPUT_BUF_SIZE: usize = 1024;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinuxWinSize {
    pub ws_row: u16,
    pub ws_col: u16,
    pub ws_xpixel: u16,
    pub ws_ypixel: u16,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinuxTermios {
    pub c_iflag: u32,
    pub c_oflag: u32,
    pub c_cflag: u32,
    pub c_lflag: u32,
    pub c_line: u8,
    pub c_cc: [u8; NCCS],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinuxTermios2 {
    pub c_iflag: u32,
    pub c_oflag: u32,
    pub c_cflag: u32,
    pub c_lflag: u32,
    pub c_line: u8,
    pub c_cc: [u8; NCCS],
    pub c_ispeed: u32,
    pub c_ospeed: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LinuxVtMode {
    pub mode: u8,
    pub waitv: u8,
    pub relsig: i16,
    pub acqsig: i16,
    pub frsig: i16,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LinuxVtState {
    pub v_active: u16,
    pub v_signal: u16,
    pub v_state: u16,
}

const _: [(); 36] = [(); size_of::<LinuxTermios>()];
const _: [(); 44] = [(); size_of::<LinuxTermios2>()];

impl LinuxWinSize {
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != size_of::<Self>() {
            return None;
        }
        Some(Self {
            ws_row: u16::from_ne_bytes(bytes[0..2].try_into().ok()?),
            ws_col: u16::from_ne_bytes(bytes[2..4].try_into().ok()?),
            ws_xpixel: u16::from_ne_bytes(bytes[4..6].try_into().ok()?),
            ws_ypixel: u16::from_ne_bytes(bytes[6..8].try_into().ok()?),
        })
    }

    pub fn to_bytes(self) -> [u8; size_of::<Self>()] {
        let mut bytes = [0u8; size_of::<Self>()];
        bytes[0..2].copy_from_slice(&self.ws_row.to_ne_bytes());
        bytes[2..4].copy_from_slice(&self.ws_col.to_ne_bytes());
        bytes[4..6].copy_from_slice(&self.ws_xpixel.to_ne_bytes());
        bytes[6..8].copy_from_slice(&self.ws_ypixel.to_ne_bytes());
        bytes
    }
}

impl LinuxTermios {
    pub fn linux_default() -> Self {
        let mut c_cc = [0u8; NCCS];
        c_cc[VINTR] = 3;
        c_cc[VQUIT] = 28;
        c_cc[VERASE] = 127;
        c_cc[VKILL] = 21;
        c_cc[VEOF] = 4;
        c_cc[VTIME] = 0;
        c_cc[VMIN] = 1;
        c_cc[VSTART] = 17;
        c_cc[VSTOP] = 19;
        c_cc[VSUSP] = 26;
        Self {
            c_iflag: ICRNL | IXON | BRKINT | ISTRIP | INPCK,
            c_oflag: OPOST | ONLCR,
            c_cflag: B38400 | CS8 | CREAD | HUPCL,
            c_lflag: ISIG | ICANON | ECHO | ECHOE | ECHOK,
            c_line: 0,
            c_cc,
        }
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != size_of::<Self>() {
            return None;
        }
        let mut c_cc = [0u8; NCCS];
        c_cc.copy_from_slice(&bytes[17..36]);
        Some(Self {
            c_iflag: u32::from_ne_bytes(bytes[0..4].try_into().ok()?),
            c_oflag: u32::from_ne_bytes(bytes[4..8].try_into().ok()?),
            c_cflag: u32::from_ne_bytes(bytes[8..12].try_into().ok()?),
            c_lflag: u32::from_ne_bytes(bytes[12..16].try_into().ok()?),
            c_line: bytes[16],
            c_cc,
        })
    }

    pub fn to_bytes(self) -> [u8; size_of::<Self>()] {
        let mut bytes = [0u8; size_of::<Self>()];
        bytes[0..4].copy_from_slice(&self.c_iflag.to_ne_bytes());
        bytes[4..8].copy_from_slice(&self.c_oflag.to_ne_bytes());
        bytes[8..12].copy_from_slice(&self.c_cflag.to_ne_bytes());
        bytes[12..16].copy_from_slice(&self.c_lflag.to_ne_bytes());
        bytes[16] = self.c_line;
        bytes[17..36].copy_from_slice(&self.c_cc);
        bytes
    }
}

impl LinuxTermios2 {
    pub fn from_termios(termios: LinuxTermios) -> Self {
        Self {
            c_iflag: termios.c_iflag,
            c_oflag: termios.c_oflag,
            c_cflag: termios.c_cflag,
            c_lflag: termios.c_lflag,
            c_line: termios.c_line,
            c_cc: termios.c_cc,
            c_ispeed: 0,
            c_ospeed: 0,
        }
    }

    pub fn into_termios(self) -> LinuxTermios {
        LinuxTermios {
            c_iflag: self.c_iflag,
            c_oflag: self.c_oflag,
            c_cflag: self.c_cflag,
            c_lflag: self.c_lflag,
            c_line: self.c_line,
            c_cc: self.c_cc,
        }
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != size_of::<Self>() {
            return None;
        }
        let mut c_cc = [0u8; NCCS];
        c_cc.copy_from_slice(&bytes[17..36]);
        Some(Self {
            c_iflag: u32::from_ne_bytes(bytes[0..4].try_into().ok()?),
            c_oflag: u32::from_ne_bytes(bytes[4..8].try_into().ok()?),
            c_cflag: u32::from_ne_bytes(bytes[8..12].try_into().ok()?),
            c_lflag: u32::from_ne_bytes(bytes[12..16].try_into().ok()?),
            c_line: bytes[16],
            c_cc,
            c_ispeed: u32::from_ne_bytes(bytes[36..40].try_into().ok()?),
            c_ospeed: u32::from_ne_bytes(bytes[40..44].try_into().ok()?),
        })
    }

    pub fn to_bytes(self) -> [u8; size_of::<Self>()] {
        let mut bytes = [0u8; size_of::<Self>()];
        bytes[0..4].copy_from_slice(&self.c_iflag.to_ne_bytes());
        bytes[4..8].copy_from_slice(&self.c_oflag.to_ne_bytes());
        bytes[8..12].copy_from_slice(&self.c_cflag.to_ne_bytes());
        bytes[12..16].copy_from_slice(&self.c_lflag.to_ne_bytes());
        bytes[16] = self.c_line;
        bytes[17..36].copy_from_slice(&self.c_cc);
        bytes[36..40].copy_from_slice(&self.c_ispeed.to_ne_bytes());
        bytes[40..44].copy_from_slice(&self.c_ospeed.to_ne_bytes());
        bytes
    }
}

impl LinuxVtMode {
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != size_of::<Self>() {
            return None;
        }
        Some(Self {
            mode: bytes[0],
            waitv: bytes[1],
            relsig: i16::from_ne_bytes(bytes[2..4].try_into().ok()?),
            acqsig: i16::from_ne_bytes(bytes[4..6].try_into().ok()?),
            frsig: i16::from_ne_bytes(bytes[6..8].try_into().ok()?),
        })
    }

    pub fn to_bytes(self) -> [u8; size_of::<Self>()] {
        let mut bytes = [0u8; size_of::<Self>()];
        bytes[0] = self.mode;
        bytes[1] = self.waitv;
        bytes[2..4].copy_from_slice(&self.relsig.to_ne_bytes());
        bytes[4..6].copy_from_slice(&self.acqsig.to_ne_bytes());
        bytes[6..8].copy_from_slice(&self.frsig.to_ne_bytes());
        bytes
    }
}

impl LinuxVtState {
    pub fn to_bytes(self) -> [u8; size_of::<Self>()] {
        let mut bytes = [0u8; size_of::<Self>()];
        bytes[0..2].copy_from_slice(&self.v_active.to_ne_bytes());
        bytes[2..4].copy_from_slice(&self.v_signal.to_ne_bytes());
        bytes[4..6].copy_from_slice(&self.v_state.to_ne_bytes());
        bytes
    }
}

static mut PROCESS_GROUP_SIGNAL_HOOK: Option<fn(i32, i32)> = None;

pub fn register_process_group_signal_hook(hook: fn(i32, i32)) {
    unsafe {
        PROCESS_GROUP_SIGNAL_HOOK = Some(hook);
    }
}

fn send_process_group_signal(process_group: i32, signal: i32) {
    let hook = unsafe { PROCESS_GROUP_SIGNAL_HOOK };
    if let Some(hook) = hook {
        hook(process_group, signal);
    }
}

pub trait TtyBackend: Send + Sync {
    fn write_bytes(&self, bytes: &[u8]);

    fn activate(&self) {}

    fn deactivate(&self) {}

    fn poll_ready(&self, events: PollEvents) -> PollEvents {
        let mut ready = PollEvents::empty();
        if events.contains(PollEvents::WRITE) {
            ready = ready | PollEvents::WRITE;
        }
        ready
    }
}

const DEFAULT_VT_COUNT: usize = 12;

pub struct FramebufferConsole {
    group: Arc<VirtualTerminalGroup>,
}

impl FramebufferConsole {
    pub fn new(surface: FramebufferSurface) -> Self {
        Self {
            group: VirtualTerminalGroup::new(surface, DEFAULT_VT_COUNT),
        }
    }

    pub fn write_bytes(&self, bytes: &[u8]) {
        self.group.active_console().write_bytes(bytes);
    }
}

impl KernelDevice for FramebufferConsole {
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn metadata(&self) -> DeviceMetadata {
        DeviceMetadata::new("tty0", DeviceClass::Console, 4, 0)
    }

    fn nodes(&self) -> alloc::vec::Vec<DeviceNode> {
        let mut nodes = alloc::vec![
            DeviceNode::new(
                "tty0",
                FileNode::new_char_device("tty0", 4, 0, self.group.active_console_file()),
            ),
            DeviceNode::new(
                default_console_alias(),
                FileNode::new_char_device(default_console_alias(), 5, 1, self.group.console_file()),
            ),
        ];

        for index in 1..=self.group.console_count() {
            nodes.push(DeviceNode::new(
                alloc::format!("tty{index}"),
                FileNode::new_char_device(
                    alloc::format!("tty{index}"),
                    4,
                    index as u32,
                    self.group.virtual_console_file(index),
                ),
            ));
        }

        nodes
    }
}

impl fmt::Write for FramebufferConsole {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.write_bytes(s.as_bytes());
        Ok(())
    }
}

impl InputEventSink for FramebufferConsole {
    fn on_input_event(&self, device: &InputDevice, event: LinuxInputEvent) {
        self.group
            .active_console_file()
            .receive_input_event(device, event);
    }
}

pub struct TtyFile {
    attachment: TtyAttachment,
}

enum TtyAttachment {
    Direct(Arc<TtyEndpoint>),
    Group {
        group: Arc<VirtualTerminalGroup>,
        selector: TtySelector,
    },
}

#[derive(Clone, Copy)]
enum TtySelector {
    Active,
    Fixed(usize),
}

#[derive(Clone, Copy)]
struct ConsoleTtyState {
    termios: LinuxTermios,
    winsize: LinuxWinSize,
    process_group: i32,
    tty_mode: i32,
    keyboard_mode: i32,
    vt_mode: LinuxVtMode,
    input_buf: [u8; TTY_INPUT_BUF_SIZE],
    input_head: u16,
    input_tail: u16,
    input_count: u16,
    canon_buf: [u8; TTY_INPUT_BUF_SIZE],
    canon_count: u16,
    key_shift: bool,
    key_ctrl: bool,
    key_alt: bool,
    key_capslock: bool,
}

struct TtyEndpoint {
    backend: Arc<dyn TtyBackend>,
    tty: SpinLock<ConsoleTtyState, LocalIrqDisabled>,
    version: AtomicU64,
    waiters: WaitQueue,
}

impl TtyEndpoint {
    fn new(backend: Arc<dyn TtyBackend>, winsize: LinuxWinSize) -> Self {
        Self {
            backend,
            tty: SpinLock::new(ConsoleTtyState {
                termios: LinuxTermios::linux_default(),
                winsize,
                process_group: 0,
                tty_mode: KD_TEXT,
                keyboard_mode: 0,
                vt_mode: LinuxVtMode::default(),
                input_buf: [0; TTY_INPUT_BUF_SIZE],
                input_head: 0,
                input_tail: 0,
                input_count: 0,
                canon_buf: [0; TTY_INPUT_BUF_SIZE],
                canon_count: 0,
                key_shift: false,
                key_ctrl: false,
                key_alt: false,
                key_capslock: false,
            }),
            version: AtomicU64::new(1),
            waiters: WaitQueue::new(),
        }
    }

    fn write_bytes(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }

        let tty_mode = self.tty.lock().tty_mode;
        if tty_mode != KD_GRAPHICS {
            self.backend.write_bytes(bytes);
        }
    }

    fn activate(&self) {
        if self.tty.lock().tty_mode != KD_GRAPHICS {
            self.backend.activate();
        }
    }

    fn deactivate(&self) {
        self.backend.deactivate();
    }

    fn bump_version(&self) {
        let _ = self.version.fetch_add(1, Ordering::AcqRel);
    }
}

fn tty_input_enqueue_byte(state: &mut ConsoleTtyState, byte: u8) -> bool {
    if state.input_count as usize >= TTY_INPUT_BUF_SIZE {
        state.input_head = (state.input_head + 1) % TTY_INPUT_BUF_SIZE as u16;
        state.input_count -= 1;
    }

    state.input_buf[state.input_tail as usize] = byte;
    state.input_tail = (state.input_tail + 1) % TTY_INPUT_BUF_SIZE as u16;
    state.input_count += 1;
    true
}

fn tty_input_dequeue_byte(state: &mut ConsoleTtyState) -> Option<u8> {
    if state.input_count == 0 {
        return None;
    }
    let byte = state.input_buf[state.input_head as usize];
    state.input_head = (state.input_head + 1) % TTY_INPUT_BUF_SIZE as u16;
    state.input_count -= 1;
    Some(byte)
}

fn tty_echo_bytes(endpoint: &TtyEndpoint, state: &ConsoleTtyState, bytes: &[u8]) {
    if bytes.is_empty() || (state.termios.c_lflag & ECHO) == 0 {
        return;
    }
    endpoint.write_bytes(bytes);
}

fn tty_echo_erase(endpoint: &TtyEndpoint, state: &ConsoleTtyState) {
    if (state.termios.c_lflag & ECHO) == 0 {
        return;
    }
    if (state.termios.c_lflag & ECHOE) != 0 {
        endpoint.write_bytes(b"\x08 \x08");
    }
}

fn tty_input_commit_canon(state: &mut ConsoleTtyState) -> bool {
    let mut committed = false;
    for index in 0..state.canon_count as usize {
        committed |= tty_input_enqueue_byte(state, state.canon_buf[index]);
    }
    state.canon_count = 0;
    committed
}

fn tty_shifted_digit(code: u16) -> Option<u8> {
    Some(match code {
        KEY_1 => b'!',
        KEY_2 => b'@',
        KEY_3 => b'#',
        KEY_4 => b'$',
        KEY_5 => b'%',
        KEY_6 => b'^',
        KEY_7 => b'&',
        KEY_8 => b'*',
        KEY_9 => b'(',
        KEY_0 => b')',
        _ => return None,
    })
}

fn tty_lookup_key_char(code: u16, shift: bool, caps: bool) -> Option<u8> {
    let byte = match code {
        KEY_A => {
            if shift ^ caps {
                b'A'
            } else {
                b'a'
            }
        }
        KEY_B => {
            if shift ^ caps {
                b'B'
            } else {
                b'b'
            }
        }
        KEY_C => {
            if shift ^ caps {
                b'C'
            } else {
                b'c'
            }
        }
        KEY_D => {
            if shift ^ caps {
                b'D'
            } else {
                b'd'
            }
        }
        KEY_E => {
            if shift ^ caps {
                b'E'
            } else {
                b'e'
            }
        }
        KEY_F => {
            if shift ^ caps {
                b'F'
            } else {
                b'f'
            }
        }
        KEY_G => {
            if shift ^ caps {
                b'G'
            } else {
                b'g'
            }
        }
        KEY_H => {
            if shift ^ caps {
                b'H'
            } else {
                b'h'
            }
        }
        KEY_I => {
            if shift ^ caps {
                b'I'
            } else {
                b'i'
            }
        }
        KEY_J => {
            if shift ^ caps {
                b'J'
            } else {
                b'j'
            }
        }
        KEY_K => {
            if shift ^ caps {
                b'K'
            } else {
                b'k'
            }
        }
        KEY_L => {
            if shift ^ caps {
                b'L'
            } else {
                b'l'
            }
        }
        KEY_M => {
            if shift ^ caps {
                b'M'
            } else {
                b'm'
            }
        }
        KEY_N => {
            if shift ^ caps {
                b'N'
            } else {
                b'n'
            }
        }
        KEY_O => {
            if shift ^ caps {
                b'O'
            } else {
                b'o'
            }
        }
        KEY_P => {
            if shift ^ caps {
                b'P'
            } else {
                b'p'
            }
        }
        KEY_Q => {
            if shift ^ caps {
                b'Q'
            } else {
                b'q'
            }
        }
        KEY_R => {
            if shift ^ caps {
                b'R'
            } else {
                b'r'
            }
        }
        KEY_S => {
            if shift ^ caps {
                b'S'
            } else {
                b's'
            }
        }
        KEY_T => {
            if shift ^ caps {
                b'T'
            } else {
                b't'
            }
        }
        KEY_U => {
            if shift ^ caps {
                b'U'
            } else {
                b'u'
            }
        }
        KEY_V => {
            if shift ^ caps {
                b'V'
            } else {
                b'v'
            }
        }
        KEY_W => {
            if shift ^ caps {
                b'W'
            } else {
                b'w'
            }
        }
        KEY_X => {
            if shift ^ caps {
                b'X'
            } else {
                b'x'
            }
        }
        KEY_Y => {
            if shift ^ caps {
                b'Y'
            } else {
                b'y'
            }
        }
        KEY_Z => {
            if shift ^ caps {
                b'Z'
            } else {
                b'z'
            }
        }
        KEY_1 => {
            if shift {
                tty_shifted_digit(code)?
            } else {
                b'1'
            }
        }
        KEY_2 => {
            if shift {
                tty_shifted_digit(code)?
            } else {
                b'2'
            }
        }
        KEY_3 => {
            if shift {
                tty_shifted_digit(code)?
            } else {
                b'3'
            }
        }
        KEY_4 => {
            if shift {
                tty_shifted_digit(code)?
            } else {
                b'4'
            }
        }
        KEY_5 => {
            if shift {
                tty_shifted_digit(code)?
            } else {
                b'5'
            }
        }
        KEY_6 => {
            if shift {
                tty_shifted_digit(code)?
            } else {
                b'6'
            }
        }
        KEY_7 => {
            if shift {
                tty_shifted_digit(code)?
            } else {
                b'7'
            }
        }
        KEY_8 => {
            if shift {
                tty_shifted_digit(code)?
            } else {
                b'8'
            }
        }
        KEY_9 => {
            if shift {
                tty_shifted_digit(code)?
            } else {
                b'9'
            }
        }
        KEY_0 => {
            if shift {
                tty_shifted_digit(code)?
            } else {
                b'0'
            }
        }
        KEY_KP0 => b'0',
        KEY_KP1 => b'1',
        KEY_KP2 => b'2',
        KEY_KP3 => b'3',
        KEY_KP4 => b'4',
        KEY_KP5 => b'5',
        KEY_KP6 => b'6',
        KEY_KP7 => b'7',
        KEY_KP8 => b'8',
        KEY_KP9 => b'9',
        _ => return None,
    };
    Some(byte)
}

fn tty_translate_key(state: &ConsoleTtyState, code: u16, out: &mut [u8; 8]) -> Option<usize> {
    let shift = state.key_shift;
    let ctrl = state.key_ctrl;
    let caps = state.key_capslock;
    let mut byte = match code {
        KEY_ENTER | KEY_KPENTER => b'\n',
        KEY_ESC => 27,
        KEY_BACKSPACE => 127,
        KEY_TAB => b'\t',
        KEY_SPACE => b' ',
        KEY_MINUS => {
            if shift {
                b'_'
            } else {
                b'-'
            }
        }
        KEY_EQUAL | KEY_KPEQUAL => {
            if shift {
                b'+'
            } else {
                b'='
            }
        }
        KEY_LEFTBRACE => {
            if shift {
                b'{'
            } else {
                b'['
            }
        }
        KEY_RIGHTBRACE => {
            if shift {
                b'}'
            } else {
                b']'
            }
        }
        KEY_BACKSLASH => {
            if shift {
                b'|'
            } else {
                b'\\'
            }
        }
        KEY_SEMICOLON => {
            if shift {
                b':'
            } else {
                b';'
            }
        }
        KEY_APOSTROPHE => {
            if shift {
                b'"'
            } else {
                b'\''
            }
        }
        KEY_GRAVE => {
            if shift {
                b'~'
            } else {
                b'`'
            }
        }
        KEY_COMMA => {
            if shift {
                b'<'
            } else {
                b','
            }
        }
        KEY_DOT => {
            if shift {
                b'>'
            } else {
                b'.'
            }
        }
        KEY_SLASH | KEY_KPSLASH => {
            if shift {
                b'?'
            } else {
                b'/'
            }
        }
        KEY_KPASTERISK => b'*',
        KEY_KPMINUS => b'-',
        KEY_KPPLUS => b'+',
        KEY_KPDOT => b'.',
        KEY_UP => return write_escape(out, b"\x1b[A"),
        KEY_DOWN => return write_escape(out, b"\x1b[B"),
        KEY_RIGHT => return write_escape(out, b"\x1b[C"),
        KEY_LEFT => return write_escape(out, b"\x1b[D"),
        KEY_HOME => return write_escape(out, b"\x1b[H"),
        KEY_END => return write_escape(out, b"\x1b[F"),
        KEY_PAGEUP => return write_escape(out, b"\x1b[5~"),
        KEY_PAGEDOWN => return write_escape(out, b"\x1b[6~"),
        KEY_INSERT => return write_escape(out, b"\x1b[2~"),
        KEY_DELETE => return write_escape(out, b"\x1b[3~"),
        _ => tty_lookup_key_char(code, shift, caps)?,
    };

    if ctrl && byte.is_ascii_alphabetic() {
        byte = (byte.to_ascii_lowercase() - b'a') + 1;
    }
    out[0] = byte;
    Some(1)
}

fn write_escape(out: &mut [u8; 8], bytes: &[u8]) -> Option<usize> {
    if bytes.len() > out.len() {
        return None;
    }
    out[..bytes.len()].copy_from_slice(bytes);
    Some(bytes.len())
}

fn tty_receive_bytes(endpoint: &TtyEndpoint, state: &mut ConsoleTtyState, bytes: &[u8]) -> bool {
    let mut wake = false;
    let canonical = (state.termios.c_lflag & ICANON) != 0;
    let eofc = state.termios.c_cc[VEOF];

    for mut byte in bytes.iter().copied() {
        if (state.termios.c_iflag & IGNCR) != 0 && byte == b'\r' {
            continue;
        }
        if (state.termios.c_iflag & ICRNL) != 0 && byte == b'\r' {
            byte = b'\n';
        } else if (state.termios.c_iflag & INLCR) != 0 && byte == b'\n' {
            byte = b'\r';
        }

        if (state.termios.c_lflag & ISIG) != 0 && state.process_group != 0 {
            if byte == state.termios.c_cc[VINTR] {
                send_process_group_signal(state.process_group, SIGINT);
                continue;
            }
            if byte == state.termios.c_cc[VQUIT] {
                send_process_group_signal(state.process_group, SIGQUIT);
                continue;
            }
            if byte == state.termios.c_cc[VSUSP] {
                send_process_group_signal(state.process_group, SIGTSTP);
                continue;
            }
        }

        if !canonical {
            wake |= tty_input_enqueue_byte(state, byte);
            tty_echo_bytes(endpoint, state, &[byte]);
            continue;
        }

        if byte == state.termios.c_cc[VERASE] || byte == 127 {
            if state.canon_count > 0 {
                state.canon_count -= 1;
                tty_echo_erase(endpoint, state);
            }
            continue;
        }

        if byte == state.termios.c_cc[VKILL] {
            while state.canon_count > 0 {
                state.canon_count -= 1;
                tty_echo_erase(endpoint, state);
            }
            if (state.termios.c_lflag & ECHOK) != 0 {
                tty_echo_bytes(endpoint, state, b"\n");
            }
            continue;
        }

        if byte == eofc {
            wake |= tty_input_commit_canon(state);
            continue;
        }

        if (state.canon_count as usize) < TTY_INPUT_BUF_SIZE {
            state.canon_buf[state.canon_count as usize] = byte;
            state.canon_count += 1;
        }
        tty_echo_bytes(endpoint, state, &[byte]);

        if byte == b'\n' || byte == state.termios.c_cc[VEOL] {
            wake |= tty_input_commit_canon(state);
        }
    }

    wake
}

pub struct VirtualTerminalGroup {
    consoles: alloc::vec::Vec<Arc<TtyEndpoint>>,
    active: Arc<AtomicUsize>,
}

impl VirtualTerminalGroup {
    fn new(surface: FramebufferSurface, count: usize) -> Arc<Self> {
        let active = Arc::new(AtomicUsize::new(1));
        let mut consoles = alloc::vec::Vec::with_capacity(count);

        for index in 1..=count {
            let plane = Arc::new(FramebufferPlane::new(surface, index, active.clone()));
            let display = TerminalDisplay {
                plane: plane.clone(),
            };
            let mut terminal = Terminal::new(display, Box::new(BitmapFont));
            terminal.set_crnl_mapping(true);
            plane.finish_init();
            let winsize = LinuxWinSize {
                ws_row: terminal.rows() as u16,
                ws_col: terminal.columns() as u16,
                ws_xpixel: surface.width().min(u16::MAX as usize) as u16,
                ws_ypixel: surface.height().min(u16::MAX as usize) as u16,
            };
            let backend: Arc<dyn TtyBackend> = Arc::new(FramebufferTtyBackend {
                terminal: SpinLock::new(terminal),
                plane,
            });
            consoles.push(Arc::new(TtyEndpoint::new(backend, winsize)));
        }

        Arc::new(Self { consoles, active })
    }

    pub fn console_count(&self) -> usize {
        self.consoles.len()
    }

    fn endpoint(&self, selector: TtySelector) -> Arc<TtyEndpoint> {
        match selector {
            TtySelector::Active => self.active_console(),
            TtySelector::Fixed(index) => self
                .consoles
                .get(index.saturating_sub(1))
                .cloned()
                .unwrap_or_else(|| self.active_console()),
        }
    }

    fn active_console(&self) -> Arc<TtyEndpoint> {
        self.consoles[self.active_index().saturating_sub(1)].clone()
    }

    fn active_index(&self) -> usize {
        self.active.load(Ordering::Acquire)
    }

    fn activate_vt(&self, index: usize) {
        let clamped = index.clamp(1, self.consoles.len().max(1));
        let previous = self.active_index();
        if previous == clamped {
            return;
        }
        self.consoles[previous - 1].deactivate();
        self.active.store(clamped, Ordering::Release);
        self.consoles[clamped - 1].activate();
    }

    fn vt_state(&self) -> LinuxVtState {
        let active = self.active_index();
        let bit_count = self.consoles.len().min(16);
        let mut state = 0u16;
        for bit in 0..bit_count {
            state |= 1u16 << bit;
        }
        LinuxVtState {
            v_active: active as u16,
            v_signal: 0,
            v_state: state,
        }
    }

    fn open_query(&self) -> i32 {
        if self.consoles.is_empty() { -1 } else { 1 }
    }

    fn active_console_file(self: &Arc<Self>) -> Arc<TtyFile> {
        Arc::new(TtyFile {
            attachment: TtyAttachment::Group {
                group: self.clone(),
                selector: TtySelector::Active,
            },
        })
    }

    fn console_file(self: &Arc<Self>) -> Arc<TtyFile> {
        self.active_console_file()
    }

    fn virtual_console_file(self: &Arc<Self>, index: usize) -> Arc<TtyFile> {
        Arc::new(TtyFile {
            attachment: TtyAttachment::Group {
                group: self.clone(),
                selector: TtySelector::Fixed(index),
            },
        })
    }
}

impl TtyFile {
    pub fn new(backend: Arc<dyn TtyBackend>, winsize: LinuxWinSize) -> Self {
        Self {
            attachment: TtyAttachment::Direct(Arc::new(TtyEndpoint::new(backend, winsize))),
        }
    }

    fn endpoint(&self) -> Arc<TtyEndpoint> {
        match &self.attachment {
            TtyAttachment::Direct(endpoint) => endpoint.clone(),
            TtyAttachment::Group { group, selector } => group.endpoint(*selector),
        }
    }

    fn group(&self) -> Option<&Arc<VirtualTerminalGroup>> {
        match &self.attachment {
            TtyAttachment::Direct(_) => None,
            TtyAttachment::Group { group, .. } => Some(group),
        }
    }

    fn with_state<R>(&self, f: impl FnOnce(&ConsoleTtyState) -> R) -> R {
        let endpoint = self.endpoint();
        let guard = endpoint.tty.lock();
        f(&guard)
    }

    fn with_state_mut<R>(&self, f: impl FnOnce(&mut ConsoleTtyState) -> R) -> R {
        let endpoint = self.endpoint();
        let mut guard = endpoint.tty.lock();
        f(&mut guard)
    }

    pub fn write_bytes(&self, bytes: &[u8]) {
        self.endpoint().write_bytes(bytes);
    }

    pub fn termios(&self) -> LinuxTermios {
        self.with_state(|state| state.termios)
    }

    pub fn set_termios(&self, termios: LinuxTermios) {
        self.with_state_mut(|state| state.termios = termios);
    }

    pub fn termios2(&self) -> LinuxTermios2 {
        LinuxTermios2::from_termios(self.termios())
    }

    pub fn set_termios2(&self, termios: LinuxTermios2) {
        self.set_termios(termios.into_termios());
    }

    pub fn winsize(&self) -> LinuxWinSize {
        self.with_state(|state| state.winsize)
    }

    pub fn set_winsize(&self, winsize: LinuxWinSize) {
        self.with_state_mut(|state| state.winsize = winsize);
    }

    pub fn process_group(&self) -> i32 {
        self.with_state(|state| state.process_group)
    }

    pub fn set_process_group(&self, process_group: i32) {
        self.with_state_mut(|state| state.process_group = process_group);
    }

    pub fn tty_mode(&self) -> i32 {
        self.with_state(|state| state.tty_mode)
    }

    pub fn set_tty_mode(&self, tty_mode: i32) {
        let endpoint = self.endpoint();
        let transition = {
            let mut guard = endpoint.tty.lock();
            let old_mode = guard.tty_mode;
            guard.tty_mode = tty_mode;
            (old_mode, tty_mode)
        };
        match transition {
            (KD_TEXT, new_mode) if new_mode != KD_TEXT => endpoint.deactivate(),
            (old_mode, KD_TEXT) if old_mode != KD_TEXT => endpoint.activate(),
            _ => {}
        }
    }

    pub fn keyboard_mode(&self) -> i32 {
        self.with_state(|state| state.keyboard_mode)
    }

    pub fn set_keyboard_mode(&self, keyboard_mode: i32) {
        self.with_state_mut(|state| state.keyboard_mode = keyboard_mode);
    }

    pub fn vt_mode(&self) -> LinuxVtMode {
        self.with_state(|state| state.vt_mode)
    }

    pub fn set_vt_mode(&self, vt_mode: LinuxVtMode) {
        self.with_state_mut(|state| state.vt_mode = vt_mode);
    }

    pub fn vt_state(&self) -> LinuxVtState {
        self.group()
            .map(|group| group.vt_state())
            .unwrap_or(LinuxVtState {
                v_active: 1,
                v_signal: 0,
                v_state: 1,
            })
    }

    pub fn set_active_vt(&self, active_vt: u16) {
        if let Some(group) = self.group() {
            group.activate_vt(active_vt as usize);
        }
    }

    pub fn open_query(&self) -> i32 {
        self.group().map_or(1, |group| group.open_query())
    }

    fn receive_input_event(&self, device: &InputDevice, event: LinuxInputEvent) {
        if !device.is_keyboard() || event.type_ != EV_KEY {
            return;
        }

        let endpoint = self.endpoint();
        let wake = {
            let mut state = endpoint.tty.lock();
            let was_empty = state.input_count == 0;
            match event.code {
                KEY_LEFTSHIFT | KEY_RIGHTSHIFT => {
                    state.key_shift = event.value != 0;
                    false
                }
                KEY_LEFTCTRL | KEY_RIGHTCTRL => {
                    state.key_ctrl = event.value != 0;
                    false
                }
                KEY_LEFTALT | KEY_RIGHTALT => {
                    state.key_alt = event.value != 0;
                    false
                }
                KEY_CAPSLOCK => {
                    if event.value == 1 {
                        state.key_capslock = !state.key_capslock;
                    }
                    false
                }
                _ => {
                    if event.value == 0 {
                        return;
                    }
                    let mut bytes = [0u8; 8];
                    let Some(len) = tty_translate_key(&state, event.code, &mut bytes) else {
                        return;
                    };
                    tty_receive_bytes(&endpoint, &mut state, &bytes[..len]) && was_empty
                }
            }
        };

        if wake {
            endpoint.bump_version();
            endpoint.waiters.notify(PollEvents::READ);
        }
    }
}

impl FileOperations for TtyFile {
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn read(&self, _offset: usize, buffer: &mut [u8]) -> FsResult<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }

        let endpoint = self.endpoint();
        let mut state = endpoint.tty.lock();
        let vmin = state.termios.c_cc[VMIN].max(1) as usize;
        let canonical = (state.termios.c_lflag & ICANON) != 0;

        if state.input_count == 0 {
            return Err(FsError::WouldBlock);
        }

        let mut read = 0usize;
        while read < buffer.len() {
            let Some(byte) = tty_input_dequeue_byte(&mut state) else {
                break;
            };
            buffer[read] = byte;
            read += 1;
            if canonical && byte == b'\n' {
                break;
            }
            if !canonical && read >= vmin {
                break;
            }
        }

        if read != 0 {
            endpoint.bump_version();
        }
        Ok(read)
    }

    fn write(&self, _offset: usize, buffer: &[u8]) -> FsResult<usize> {
        if let Ok(str) = str::from_utf8(buffer) {
            serial_print!("{}", str);
        }
        self.write_bytes(buffer);
        Ok(buffer.len())
    }

    fn ioctl(&self, command: u64, _argument: u64) -> FsResult<IoctlResponse> {
        match command {
            TIOCGWINSZ => Ok(IoctlResponse::Data(self.winsize().to_bytes().to_vec())),
            TCGETS => Ok(IoctlResponse::Data(self.termios().to_bytes().to_vec())),
            TCGETS2 => Ok(IoctlResponse::Data(self.termios2().to_bytes().to_vec())),
            TIOCGPGRP => Ok(IoctlResponse::Data(
                self.process_group().to_ne_bytes().to_vec(),
            )),
            KDGETMODE => Ok(IoctlResponse::Data(self.tty_mode().to_ne_bytes().to_vec())),
            KDGKBMODE => Ok(IoctlResponse::Data(
                self.keyboard_mode().to_ne_bytes().to_vec(),
            )),
            VT_GETMODE => Ok(IoctlResponse::Data(self.vt_mode().to_bytes().to_vec())),
            VT_GETSTATE => Ok(IoctlResponse::Data(self.vt_state().to_bytes().to_vec())),
            VT_OPENQRY => Ok(IoctlResponse::Data(
                self.open_query().to_ne_bytes().to_vec(),
            )),
            _ => Err(aether_vfs::FsError::Unsupported),
        }
    }

    fn poll(&self, events: PollEvents) -> FsResult<PollEvents> {
        let endpoint = self.endpoint();
        let mut ready = endpoint.backend.poll_ready(events);
        let state = endpoint.tty.lock();
        if events.contains(PollEvents::READ) && state.input_count != 0 {
            ready = ready | PollEvents::READ;
        }
        Ok(ready)
    }

    fn wait_token(&self) -> u64 {
        self.endpoint().version.load(Ordering::Acquire)
    }

    fn register_waiter(
        &self,
        events: PollEvents,
        listener: SharedWaitListener,
    ) -> FsResult<Option<u64>> {
        Ok(Some(self.endpoint().waiters.register(events, listener)))
    }

    fn unregister_waiter(&self, waiter_id: u64) -> FsResult<()> {
        let _ = self.endpoint().waiters.unregister(waiter_id);
        Ok(())
    }
}

pub type ConsoleCore = TtyFile;

struct FramebufferPlane {
    surface: FramebufferSurface,
    active: Arc<AtomicUsize>,
    index: usize,
    width: usize,
    height: usize,
    initialized: AtomicBool,
    pixels: Once<alloc::boxed::Box<[AtomicU32]>>,
}

impl FramebufferPlane {
    fn new(surface: FramebufferSurface, index: usize, active: Arc<AtomicUsize>) -> Self {
        let width = surface.width();
        let height = surface.height();
        Self {
            surface,
            active,
            index,
            width,
            height,
            initialized: AtomicBool::new(false),
            pixels: Once::new(),
        }
    }

    fn finish_init(&self) {
        self.initialized.store(true, Ordering::Release);
    }

    fn pixels(&self) -> &[AtomicU32] {
        self.pixels.call_once(|| {
            let black = self.surface.pack_color(RgbColor::BLACK);
            (0..self.width.saturating_mul(self.height))
                .map(|_| AtomicU32::new(black))
                .collect::<alloc::vec::Vec<_>>()
                .into_boxed_slice()
        })
    }

    fn packed_pixel(&self, x: usize, y: usize) -> u32 {
        let bytes_per_pixel = self.surface.bytes_per_pixel();
        let mut bytes = [0u8; 4];
        let offset = y
            .saturating_mul(self.surface.stride())
            .saturating_add(x.saturating_mul(bytes_per_pixel));
        let _ = self
            .surface
            .read_bytes(offset, &mut bytes[..bytes_per_pixel]);
        u32::from_le_bytes(bytes)
    }

    fn snapshot_surface(&self) {
        if self.pixels.get().is_some() {
            return;
        }
        let _ = self.pixels.call_once(|| {
            let mut pixels = alloc::vec::Vec::with_capacity(self.width.saturating_mul(self.height));
            for y in 0..self.height {
                for x in 0..self.width {
                    pixels.push(AtomicU32::new(self.packed_pixel(x, y)));
                }
            }
            pixels.into_boxed_slice()
        });
    }

    fn offset(&self, x: usize, y: usize) -> Option<usize> {
        (x < self.width && y < self.height).then_some(y * self.width + x)
    }

    fn draw_pixel(&self, x: usize, y: usize, color: RgbColor) {
        let Some(offset) = self.offset(x, y) else {
            return;
        };
        let packed = self.surface.pack_color(color);
        let active = self.active.load(Ordering::Acquire) == self.index;
        if !self.initialized.load(Ordering::Acquire) {
            if active {
                let _ = self.surface.write_packed_pixel(x, y, packed);
            }
            return;
        }
        if active {
            if let Some(pixels) = self.pixels.get() {
                pixels[offset].store(packed, Ordering::Relaxed);
            }
            let _ = self.surface.write_packed_pixel(x, y, packed);
            return;
        }
        self.pixels()[offset].store(packed, Ordering::Relaxed);
    }

    fn redraw(&self) {
        let Some(pixels) = self.pixels.get() else {
            return;
        };
        for y in 0..self.height {
            for x in 0..self.width {
                let packed = pixels[y * self.width + x].load(Ordering::Relaxed);
                let _ = self.surface.write_packed_pixel(x, y, packed);
            }
        }
    }
}

struct FramebufferTtyBackend {
    terminal: SpinLock<Terminal<TerminalDisplay>>,
    plane: Arc<FramebufferPlane>,
}

impl TtyBackend for FramebufferTtyBackend {
    fn write_bytes(&self, bytes: &[u8]) {
        let mut terminal = self.terminal.lock();
        terminal.process(bytes);
    }

    fn activate(&self) {
        self.plane.redraw();
    }

    fn deactivate(&self) {
        self.plane.snapshot_surface();
    }
}

#[derive(Clone)]
struct TerminalDisplay {
    plane: Arc<FramebufferPlane>,
}

impl DrawTarget for TerminalDisplay {
    fn size(&self) -> (usize, usize) {
        (self.plane.width, self.plane.height)
    }

    #[inline(always)]
    fn draw_pixel(&mut self, x: usize, y: usize, color: Rgb) {
        self.plane.draw_pixel(
            x,
            y,
            RgbColor {
                red: color.0,
                green: color.1,
                blue: color.2,
            },
        );
    }
}
