#![no_std]

extern crate alloc;

use aether_frame::serial_print;
use alloc::boxed::Box;
use alloc::sync::Arc;
use core::fmt;
use core::mem::size_of;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use aether_device::{DeviceClass, DeviceMetadata, DeviceNode, KernelDevice, default_console_alias};
use aether_frame::libs::spin::SpinLock;
use aether_framebuffer::{FramebufferSurface, RgbColor};
use aether_vfs::{FileNode, FileOperations, FsResult, IoctlResponse, PollEvents};
use os_terminal::font::BitmapFont;
use os_terminal::{DrawTarget, Rgb, Terminal};

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

const BRKINT: u32 = 0o000002;
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

pub trait TtyBackend: Send + Sync {
    fn write_bytes(&self, bytes: &[u8]);

    fn activate(&self) {}

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
}

struct TtyEndpoint {
    backend: Arc<dyn TtyBackend>,
    tty: SpinLock<ConsoleTtyState>,
}

impl TtyEndpoint {
    fn new(backend: Arc<dyn TtyBackend>, winsize: LinuxWinSize) -> Self {
        Self {
            backend,
            tty: SpinLock::new(ConsoleTtyState {
                termios: LinuxTermios::linux_default(),
                winsize,
                process_group: 0,
                tty_mode: 0,
                keyboard_mode: 0,
                vt_mode: LinuxVtMode::default(),
            }),
        }
    }

    fn write_bytes(&self, bytes: &[u8]) {
        self.backend.write_bytes(bytes);
    }

    fn activate(&self) {
        self.backend.activate();
    }
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

        let group = Arc::new(Self { consoles, active });
        group.active_console().activate();
        group
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
        let guard = endpoint.tty.lock_irqsave();
        f(&guard)
    }

    fn with_state_mut<R>(&self, f: impl FnOnce(&mut ConsoleTtyState) -> R) -> R {
        let endpoint = self.endpoint();
        let mut guard = endpoint.tty.lock_irqsave();
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
        self.with_state_mut(|state| state.tty_mode = tty_mode);
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
}

impl FileOperations for TtyFile {
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn read(&self, _offset: usize, _buffer: &mut [u8]) -> FsResult<usize> {
        Ok(0)
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
        Ok(self.endpoint().backend.poll_ready(events))
    }
}

pub type ConsoleCore = TtyFile;

struct FramebufferPlane {
    surface: FramebufferSurface,
    active: Arc<AtomicUsize>,
    index: usize,
    width: usize,
    height: usize,
    pixels: alloc::vec::Vec<AtomicU32>,
}

impl FramebufferPlane {
    fn new(surface: FramebufferSurface, index: usize, active: Arc<AtomicUsize>) -> Self {
        let width = surface.width();
        let height = surface.height();
        let black = surface.pack_color(RgbColor::BLACK);
        let pixels = (0..width.saturating_mul(height))
            .map(|_| AtomicU32::new(black))
            .collect();
        Self {
            surface,
            active,
            index,
            width,
            height,
            pixels,
        }
    }

    fn offset(&self, x: usize, y: usize) -> Option<usize> {
        (x < self.width && y < self.height).then_some(y * self.width + x)
    }

    fn draw_pixel(&self, x: usize, y: usize, color: RgbColor) {
        let Some(offset) = self.offset(x, y) else {
            return;
        };
        let packed = self.surface.pack_color(color);
        self.pixels[offset].store(packed, Ordering::Relaxed);
        if self.active.load(Ordering::Acquire) == self.index {
            let _ = self.surface.write_packed_pixel(x, y, packed);
        }
    }

    fn redraw(&self) {
        for y in 0..self.height {
            for x in 0..self.width {
                let packed = self.pixels[y * self.width + x].load(Ordering::Relaxed);
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
        let mut terminal = self.terminal.lock_irqsave();
        terminal.process(bytes);
    }

    fn activate(&self) {
        self.plane.redraw();
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
