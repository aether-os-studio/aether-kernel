extern crate alloc;

use aether_device::{DeviceRegistry, KernelDevice};
use aether_frame::acpi;
use aether_frame::interrupt::{self, Trap, TrapFrame, device::allocate_vector};
use aether_frame::io::Port;
use aether_frame::libs::spin::SpinLock;
use alloc::string::ToString;
use alloc::sync::Arc;

use crate::input::{
    BTN_LEFT, BTN_MIDDLE, BTN_RIGHT, BUS_I8042, EV_KEY, EV_REL, EV_REP, EV_SYN, InputDevice,
    InputDeviceDescriptor, InputTopology, KEY_ESC, KEY_MENU, REL_WHEEL, REL_X, REL_Y, SYN_REPORT,
    evdev_code_from_set1_scancode,
};

const PS2_DATA_PORT: u16 = 0x60;
const PS2_STATUS_PORT: u16 = 0x64;
const PS2_COMMAND_PORT: u16 = 0x64;

const PS2_CMD_READ_CONFIG: u8 = 0x20;
const PS2_CMD_WRITE_CONFIG: u8 = 0x60;
const PS2_CMD_DISABLE_PORT2: u8 = 0xA7;
const PS2_CMD_ENABLE_PORT2: u8 = 0xA8;
const PS2_CMD_TEST_PORT2: u8 = 0xA9;
const PS2_CMD_TEST_CONTROLLER: u8 = 0xAA;
const PS2_CMD_TEST_PORT1: u8 = 0xAB;
const PS2_CMD_DISABLE_PORT1: u8 = 0xAD;
const PS2_CMD_ENABLE_PORT1: u8 = 0xAE;
const PS2_CMD_WRITE_PORT2: u8 = 0xD4;

const PS2_DEV_RESET: u8 = 0xFF;
const PS2_DEV_ENABLE: u8 = 0xF4;
const PS2_DEV_SET_DEFAULTS: u8 = 0xF6;
const PS2_DEV_IDENTIFY: u8 = 0xF2;
const PS2_DEV_SET_SAMPLE_RATE: u8 = 0xF3;

const PS2_ACK: u8 = 0xFA;
const PS2_RESEND: u8 = 0xFE;
const PS2_IO_TIMEOUT_NS: u64 = 50_000_000;
const PS2_RESET_TIMEOUT_NS: u64 = 500_000_000;

static CONTROLLER: SpinLock<Option<Arc<I8042Controller>>> = SpinLock::new(None);

pub fn probe(registry: &mut DeviceRegistry) {
    #[cfg(target_arch = "x86_64")]
    {
        if !acpi::info().motherboard_implements_8042() {
            log::info!("i8042: skipping probe because ACPI reports no 8042 controller");
            return;
        }
        match I8042Controller::probe(registry) {
            Ok(controller) => {
                *CONTROLLER.lock() = Some(controller);
            }
            Err(error) => {
                log::warn!("i8042: probe failed: {error}");
            }
        }
    }

    #[cfg(not(target_arch = "x86_64"))]
    let _ = registry;
}

struct I8042State {
    keyboard_extended: bool,
    mouse_cycle: usize,
    mouse_packet: [u8; 4],
    mouse_has_wheel: bool,
    mouse_left_pressed: bool,
    mouse_right_pressed: bool,
    mouse_middle_pressed: bool,
    port1_available: bool,
    port2_available: bool,
}

struct I8042Controller {
    state: SpinLock<I8042State>,
    keyboard: Arc<InputDevice>,
    mouse: Option<Arc<InputDevice>>,
}

impl I8042Controller {
    fn probe(registry: &mut DeviceRegistry) -> Result<Arc<Self>, &'static str> {
        let keyboard = Arc::new(new_keyboard_device());
        let mouse = Arc::new(new_mouse_device());
        let controller = Arc::new(Self {
            state: SpinLock::new(I8042State {
                keyboard_extended: false,
                mouse_cycle: 0,
                mouse_packet: [0; 4],
                mouse_has_wheel: false,
                mouse_left_pressed: false,
                mouse_right_pressed: false,
                mouse_middle_pressed: false,
                port1_available: false,
                port2_available: false,
            }),
            keyboard: keyboard.clone(),
            mouse: Some(mouse.clone()),
        });

        controller.init_controller()?;
        controller.init_keyboard()?;
        let mouse_enabled = controller.init_mouse().is_ok();

        let keyboard_device: Arc<dyn KernelDevice> = keyboard;
        registry.register(keyboard_device);
        if mouse_enabled {
            let mouse_device: Arc<dyn KernelDevice> = mouse;
            registry.register(mouse_device);
        } else {
            controller.state.lock().port2_available = false;
        }

        Ok(controller)
    }

    fn init_controller(&self) -> Result<(), &'static str> {
        write_command(PS2_CMD_DISABLE_PORT1);
        write_command(PS2_CMD_DISABLE_PORT2);
        flush_output_buffer();

        write_command(PS2_CMD_READ_CONFIG);
        let mut config = read_data();
        config &= !0x43;
        let is_dual_channel = (config & 0x20) != 0;
        write_command(PS2_CMD_WRITE_CONFIG);
        write_data(config);

        write_command(PS2_CMD_TEST_CONTROLLER);
        if read_data_timeout(PS2_RESET_TIMEOUT_NS) != 0x55 {
            return Err("controller self-test failed");
        }

        let mut state = self.state.lock();
        if is_dual_channel {
            write_command(PS2_CMD_ENABLE_PORT2);
            write_command(PS2_CMD_READ_CONFIG);
            config = read_data();
            if (config & 0x20) == 0 {
                state.port2_available = true;
                write_command(PS2_CMD_DISABLE_PORT2);
            }
        }
        write_command(PS2_CMD_TEST_PORT1);
        state.port1_available = read_data_timeout(PS2_RESET_TIMEOUT_NS) == 0x00;
        if state.port2_available {
            write_command(PS2_CMD_TEST_PORT2);
            state.port2_available = read_data_timeout(PS2_RESET_TIMEOUT_NS) == 0x00;
        }
        if !state.port1_available && !state.port2_available {
            return Err("no PS/2 ports available");
        }
        Ok(())
    }

    fn init_keyboard(&self) -> Result<(), &'static str> {
        if !self.state.lock().port1_available {
            return Err("keyboard port unavailable");
        }

        install_isa_irq(1)?;
        flush_output_buffer();

        write_command(PS2_CMD_ENABLE_PORT1);
        if !send_to_port1(PS2_DEV_RESET) {
            return Err("keyboard reset failed");
        }
        if read_data_timeout(PS2_RESET_TIMEOUT_NS) != 0xAA {
            return Err("keyboard self-test failed");
        }
        if !send_to_port1(PS2_DEV_ENABLE) {
            return Err("keyboard enable failed");
        }
        if !send_to_port1(0xF0) || !send_to_port1(0x01) {
            return Err("failed to select set1 scan codes");
        }

        write_command(PS2_CMD_READ_CONFIG);
        let mut config = read_data();
        config |= 0x01;
        write_command(PS2_CMD_WRITE_CONFIG);
        write_data(config);
        Ok(())
    }

    fn init_mouse(&self) -> Result<(), &'static str> {
        if !self.state.lock().port2_available {
            return Err("mouse port unavailable");
        }

        install_isa_irq(12)?;
        flush_output_buffer();

        write_command(PS2_CMD_ENABLE_PORT2);
        if !send_to_port2(PS2_DEV_RESET) {
            return Err("mouse reset failed");
        }
        if read_data_timeout(PS2_RESET_TIMEOUT_NS) != 0xAA {
            return Err("mouse self-test failed");
        }
        let _ = read_data();

        {
            let mut state = self.state.lock();
            state.mouse_has_wheel = mouse_detect_wheel();
        }

        if !send_to_port2(PS2_DEV_SET_DEFAULTS) || !send_to_port2(PS2_DEV_ENABLE) {
            return Err("mouse enable failed");
        }

        write_command(PS2_CMD_READ_CONFIG);
        let mut config = read_data();
        config |= 0x02;
        write_command(PS2_CMD_WRITE_CONFIG);
        write_data(config);
        Ok(())
    }

    fn handle_interrupt(&self) {
        while let Some((status, data)) = read_pending_data() {
            if (status & 0x20) != 0 {
                self.handle_mouse_data(data);
            } else {
                self.handle_keyboard_data(data);
            }
        }
    }

    fn handle_keyboard_data(&self, data: u8) {
        let (scan_code, pressed, is_extended) = {
            let mut state = self.state.lock();
            if data == 0xE0 {
                state.keyboard_extended = true;
                return;
            }
            let is_extended = state.keyboard_extended;
            state.keyboard_extended = false;
            (data & 0x7f, (data & 0x80) == 0, is_extended)
        };

        let code = evdev_code_from_set1_scancode(scan_code, is_extended);
        if code == 0 {
            return;
        }
        let events = [
            self.keyboard
                .emit_event_spec(EV_KEY, code, if pressed { 1 } else { 0 }),
            self.keyboard.emit_event_spec(EV_SYN, SYN_REPORT, 0),
        ];
        self.keyboard.emit_events(&events);
    }

    fn handle_mouse_data(&self, data: u8) {
        let packet = {
            let mut state = self.state.lock();
            let packet_size = if state.mouse_has_wheel { 4 } else { 3 };
            let cycle = state.mouse_cycle;
            state.mouse_packet[cycle] = data;
            state.mouse_cycle += 1;
            if state.mouse_cycle < packet_size {
                return;
            }
            state.mouse_cycle = 0;
            if (state.mouse_packet[0] & 0x08) == 0 {
                return;
            }
            state.mouse_packet
        };

        let Some(mouse) = self.mouse.as_ref() else {
            return;
        };

        let left = (packet[0] & 0x01) != 0;
        let right = (packet[0] & 0x02) != 0;
        let middle = (packet[0] & 0x04) != 0;
        let mut x = packet[1] as i16;
        if (packet[0] & 0x10) != 0 {
            x |= !0xff;
        }
        let mut y = packet[2] as i16;
        if (packet[0] & 0x20) != 0 {
            y |= !0xff;
        }
        y = -y;
        let wheel = if self.state.lock().mouse_has_wheel {
            let mut z = (packet[3] & 0x0f) as i8;
            if (z & 0x08) != 0 {
                z |= !0x0f;
            }
            z
        } else {
            0
        };

        let mut events = [crate::input::LinuxInputEvent::default(); 7];
        let mut count = 0usize;
        let mut emitted = false;
        if x != 0 {
            events[count] = mouse.emit_event_spec(EV_REL, REL_X, x as i32);
            count += 1;
            emitted = true;
        }
        if y != 0 {
            events[count] = mouse.emit_event_spec(EV_REL, REL_Y, y as i32);
            count += 1;
            emitted = true;
        }
        if wheel != 0 {
            events[count] = mouse.emit_event_spec(EV_REL, REL_WHEEL, -(wheel as i32));
            count += 1;
            emitted = true;
        }

        {
            let mut state = self.state.lock();
            if state.mouse_left_pressed != left {
                state.mouse_left_pressed = left;
                events[count] = mouse.emit_event_spec(EV_KEY, BTN_LEFT, if left { 1 } else { 0 });
                count += 1;
                emitted = true;
            }
            if state.mouse_right_pressed != right {
                state.mouse_right_pressed = right;
                events[count] = mouse.emit_event_spec(EV_KEY, BTN_RIGHT, if right { 1 } else { 0 });
                count += 1;
                emitted = true;
            }
            if state.mouse_middle_pressed != middle {
                state.mouse_middle_pressed = middle;
                events[count] =
                    mouse.emit_event_spec(EV_KEY, BTN_MIDDLE, if middle { 1 } else { 0 });
                count += 1;
                emitted = true;
            }
        }

        if emitted {
            events[count] = mouse.emit_event_spec(EV_SYN, SYN_REPORT, 0);
            count += 1;
            mouse.emit_events(&events[..count]);
        }
    }
}

fn new_keyboard_device() -> InputDevice {
    let mut descriptor = InputDeviceDescriptor::new("AT Translated Set 2 keyboard")
        .with_phys("isa0060/serio0/input0")
        .with_input_id(crate::input::LinuxInputId {
            bustype: BUS_I8042,
            vendor: 0x0001,
            product: 0x0001,
            version: 0xab41,
        });
    descriptor.set_event(EV_REP);
    for key in KEY_ESC..=KEY_MENU {
        descriptor.set_key(key);
    }
    InputDevice::new(
        InputTopology::PlatformDevice {
            device: "i8042".to_string(),
            child: Some("serio0".to_string()),
        },
        descriptor,
    )
    .as_ref()
    .clone()
}

fn new_mouse_device() -> InputDevice {
    let mut descriptor = InputDeviceDescriptor::new("PS/2 Generic Mouse")
        .with_phys("isa0060/serio1/input0")
        .with_input_id(crate::input::LinuxInputId {
            bustype: BUS_I8042,
            vendor: 0x0002,
            product: 0x0001,
            version: 0x0001,
        });
    descriptor.set_property(crate::input::INPUT_PROP_POINTER);
    descriptor.set_key(BTN_LEFT);
    descriptor.set_key(BTN_RIGHT);
    descriptor.set_key(BTN_MIDDLE);
    descriptor.set_rel(REL_X);
    descriptor.set_rel(REL_Y);
    descriptor.set_rel(REL_WHEEL);
    InputDevice::new(
        InputTopology::PlatformDevice {
            device: "i8042".to_string(),
            child: Some("serio1".to_string()),
        },
        descriptor,
    )
    .as_ref()
    .clone()
}

fn ps2_interrupt_handler(_trap: Trap, _frame: &mut TrapFrame) {
    let Some(controller) = CONTROLLER.lock().as_ref().cloned() else {
        return;
    };
    controller.handle_interrupt();
}

fn install_isa_irq(irq: u8) -> Result<(), &'static str> {
    let vector = allocate_vector().map_err(|_| "no device vectors left")?;
    interrupt::register_handler(vector, ps2_interrupt_handler)
        .map_err(|_| "failed to register i8042 interrupt handler")?;
    let lapic = interrupt::current_lapic_id().ok_or("no current LAPIC id")?;
    aether_frame::arch::interrupt::ioapic::configure_isa_irq(irq, vector, lapic as u8)
}

fn mouse_detect_wheel() -> bool {
    mouse_set_sample_rate(200) && mouse_set_sample_rate(100) && mouse_set_sample_rate(80) && {
        send_to_port2(PS2_DEV_IDENTIFY) && matches!(read_data(), 3 | 4)
    }
}

fn mouse_set_sample_rate(rate: u8) -> bool {
    send_to_port2(PS2_DEV_SET_SAMPLE_RATE) && send_to_port2(rate)
}

fn wait_write() -> bool {
    wait_write_timeout(PS2_IO_TIMEOUT_NS)
}

fn wait_write_timeout(timeout_ns: u64) -> bool {
    let deadline = aether_frame::time::monotonic_nanos().saturating_add(timeout_ns);
    while aether_frame::time::monotonic_nanos() < deadline {
        if (read_status() & 0x02) == 0 {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

fn wait_read() -> bool {
    wait_read_timeout(PS2_IO_TIMEOUT_NS)
}

fn wait_read_timeout(timeout_ns: u64) -> bool {
    let deadline = aether_frame::time::monotonic_nanos().saturating_add(timeout_ns);
    while aether_frame::time::monotonic_nanos() < deadline {
        if (read_status() & 0x01) != 0 {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

fn read_status() -> u8 {
    unsafe { Port::<u8>::new(PS2_STATUS_PORT) }.read()
}

fn read_data() -> u8 {
    read_data_timeout(PS2_IO_TIMEOUT_NS)
}

fn read_data_timeout(timeout_ns: u64) -> u8 {
    if !wait_read_timeout(timeout_ns) {
        return 0xff;
    }
    read_data_nowait()
}

fn read_data_nowait() -> u8 {
    unsafe { Port::<u8>::new(PS2_DATA_PORT) }.read()
}

fn read_pending_data() -> Option<(u8, u8)> {
    let status = read_status();
    if (status & 0x01) == 0 {
        return None;
    }
    Some((status, read_data_nowait()))
}

fn flush_output_buffer() {
    loop {
        if read_pending_data().is_none() {
            break;
        }
    }
}

fn write_command(command: u8) {
    if wait_write() {
        unsafe { Port::<u8>::new(PS2_COMMAND_PORT) }.write(command);
    }
}

fn write_data(data: u8) {
    if wait_write() {
        unsafe { Port::<u8>::new(PS2_DATA_PORT) }.write(data);
    }
}

fn send_to_port1(command: u8) -> bool {
    for _ in 0..3 {
        write_data(command);
        match read_data() {
            PS2_ACK => return true,
            PS2_RESEND => continue,
            _ => break,
        }
    }
    false
}

fn send_to_port2(command: u8) -> bool {
    for _ in 0..3 {
        write_command(PS2_CMD_WRITE_PORT2);
        write_data(command);
        match read_data() {
            PS2_ACK => return true,
            PS2_RESEND => continue,
            _ => break,
        }
    }
    false
}
