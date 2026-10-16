use std::net::{IpAddr, Ipv4Addr};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Instant;

use tokio::sync::mpsc;

use crate::conn::{self, Command};
use crate::proto::tcp::{TcpPacket, TcpTag};
use crate::proto::udp::{
    Alliance, Control, ControlPacket, JoystickTag, Request, Status, Trace, UdpTag,
};
use crate::{Error, Result};

pub(crate) type JoystickSupplier = dyn Fn() -> Vec<Vec<JoystickValue>> + Send + Sync + 'static;
pub(crate) type TcpConsumer = dyn FnMut(TcpPacket) + Send + 'static;

const USB_IP: Ipv4Addr = Ipv4Addr::new(172, 22, 11, 2);

/// Returns the conventional roboRIO address for a team, `10.TE.AM.2`
pub fn ip_from_team_number(team: u32) -> IpAddr {
    IpAddr::V4(Ipv4Addr::new(
        10,
        (team / 100).min(255) as u8,
        (team % 100) as u8,
        2,
    ))
}

/// The robot mode, considered separately from whether the robot is enabled
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    Autonomous,
    Teleoperated,
    Test,
}

/// Whether the driver station is talking to a real robot or the WPILib simulator.
///
/// Simulation mode is entered automatically while a simulator is announcing itself on
/// `127.0.0.1:1135`, and redirects all traffic to localhost.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum DsMode {
    Normal,
    Simulation,
}

/// A value from a joystick to be transmitted to the roboRIO
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum JoystickValue {
    /// Axis `id` (0-5). `value` ranges `-1.0..=1.0`, or `0.0..=1.0` for triggers
    Axis { id: u8, value: f32 },
    /// Button `id` (1-10)
    Button { id: u8, pressed: bool },
    /// POV `id` (0), `angle` in degrees or -1 when released
    POV { id: u8, angle: i16 },
}

/// Ports used to communicate with the robot. Only tests need to change these.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Ports {
    /// UDP port the roboRIO listens on for control packets
    pub robot_udp: u16,
    /// UDP port the driver station listens on for status packets
    pub ds_udp: u16,
    /// TCP port on the roboRIO for console output and game data
    pub robot_tcp: u16,
    /// UDP port the WPILib simulator announces itself on. `None` disables simulator detection
    pub sim_detect: Option<u16>,
}

impl Default for Ports {
    fn default() -> Self {
        Ports {
            robot_udp: 1110,
            ds_udp: 1150,
            robot_tcp: 1740,
            sim_detect: Some(1135),
        }
    }
}

/// A snapshot of what the robot last reported
#[derive(Debug, Copy, Clone, PartialEq, Default)]
pub struct Telemetry {
    /// Whether status packets are arriving from the robot
    pub connected: bool,
    /// Whether user code is running
    pub code_running: bool,
    pub voltage: f32,
    pub brownout: bool,
    pub trace: Trace,
    /// When the last status packet arrived
    pub last_packet: Option<Instant>,
}

pub(crate) struct ControlState {
    pub mode: Mode,
    pub enabled: bool,
    pub estopped: bool,
    pub alliance: Alliance,
    pub request: Request,
    pub seqnum: u16,
    pub pending: Vec<UdpTag>,
    pub team_number: u32,
    pub use_usb: bool,
    pub ip_override: Option<IpAddr>,
    pub ds_mode: DsMode,
    pub game_data: Option<String>,
}

impl ControlState {
    /// The address control packets are currently sent to
    pub fn target_ip(&self) -> IpAddr {
        if self.ds_mode == DsMode::Simulation {
            IpAddr::V4(Ipv4Addr::LOCALHOST)
        } else if self.use_usb {
            IpAddr::V4(USB_IP)
        } else if let Some(ip) = self.ip_override {
            ip
        } else {
            ip_from_team_number(self.team_number)
        }
    }

    /// Called whenever the connection target changes or the link drops.
    ///
    /// The e-stop latch is released too: a roboRIO stays e-stopped until it reboots and keeps
    /// reporting it, so if it is still e-stopped the next status packet latches it again.
    pub fn reset_link(&mut self) {
        self.seqnum = 0;
        self.enabled = false;
        self.estopped = false;
    }
}

pub(crate) struct Shared {
    pub control: Mutex<ControlState>,
    pub telemetry: Mutex<Telemetry>,
    pub joystick_supplier: Mutex<Option<Box<JoystickSupplier>>>,
    pub tcp_consumer: Mutex<Option<Box<TcpConsumer>>>,
    /// Why the driver station can't currently receive from the robot, if it can't
    pub link_error: Mutex<Option<String>>,
}

impl Shared {
    pub fn control(&self) -> MutexGuard<'_, ControlState> {
        self.control.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn telemetry(&self) -> MutexGuard<'_, Telemetry> {
        self.telemetry.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Builds the next control packet, consuming any one-shot requests and tags.
    pub fn next_packet(&self) -> ControlPacket {
        // Called outside the control lock so that suppliers may freely query the driver station
        let joysticks = self
            .joystick_supplier
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|supplier| supplier())
            .unwrap_or_default();

        let mut control = self.control();
        let mut bits = control.mode.to_control();
        if control.enabled && !control.estopped {
            bits |= Control::ENABLED;
        }
        if control.estopped {
            bits |= Control::ESTOP;
        }

        let mut tags = std::mem::take(&mut control.pending);
        tags.extend(
            joysticks
                .iter()
                .map(|values| UdpTag::Joystick(joystick_tag(values))),
        );

        let packet = ControlPacket {
            seqnum: control.seqnum,
            control: bits,
            request: std::mem::replace(&mut control.request, Request::empty()),
            alliance: control.alliance,
            tags,
        };
        control.seqnum = control.seqnum.wrapping_add(1);
        packet
    }
}

/// Converts supplied values into the fixed 6 axis / 10 button / 1 POV layout.
/// An empty slice means nothing is plugged into that slot.
fn joystick_tag(values: &[JoystickValue]) -> JoystickTag {
    if values.is_empty() {
        return JoystickTag::default();
    }
    let mut tag = JoystickTag {
        axes: vec![0; 6],
        buttons: vec![false; 10],
        povs: vec![-1],
    };
    for value in values {
        match *value {
            JoystickValue::Axis { id, value } => {
                if let Some(axis) = tag.axes.get_mut(id as usize) {
                    *axis = crate::proto::udp::encode_axis(value);
                }
            }
            JoystickValue::Button { id, pressed } => {
                if let Some(button) = (id as usize)
                    .checked_sub(1)
                    .and_then(|i| tag.buttons.get_mut(i))
                {
                    *button = pressed;
                }
            }
            JoystickValue::POV { id, angle } => {
                if let Some(pov) = tag.povs.get_mut(id as usize) {
                    *pov = angle;
                }
            }
        }
    }
    tag
}

/// Configures a [`DriverStation`] before connecting
pub struct Builder {
    team_number: u32,
    alliance: Alliance,
    ip_override: Option<IpAddr>,
    ports: Ports,
}

impl Builder {
    pub fn team_number(mut self, team: u32) -> Self {
        self.team_number = team;
        self
    }

    pub fn alliance(mut self, alliance: Alliance) -> Self {
        self.alliance = alliance;
        self
    }

    /// Connect to `ip` instead of the address derived from the team number
    pub fn target_ip(mut self, ip: IpAddr) -> Self {
        self.ip_override = Some(ip);
        self
    }

    pub fn ports(mut self, ports: Ports) -> Self {
        self.ports = ports;
        self
    }

    /// Spawns the I/O thread and starts talking to the robot
    pub fn connect(self) -> DriverStation {
        let shared = Arc::new(Shared {
            control: Mutex::new(ControlState {
                mode: Mode::Autonomous,
                enabled: false,
                estopped: false,
                alliance: self.alliance,
                request: Request::empty(),
                seqnum: 0,
                pending: Vec::new(),
                team_number: self.team_number,
                use_usb: false,
                ip_override: self.ip_override,
                ds_mode: DsMode::Normal,
                game_data: None,
            }),
            telemetry: Mutex::new(Telemetry::default()),
            joystick_supplier: Mutex::new(None),
            tcp_consumer: Mutex::new(None),
            link_error: Mutex::new(None),
        });
        let (tx, rx) = mpsc::unbounded_channel();
        let thread = conn::spawn(shared.clone(), rx, self.ports);
        DriverStation {
            shared,
            commands: tx,
            thread: Some(thread),
        }
    }
}

/// A connection to a roboRIO, acting as its driver station.
///
/// All I/O happens on a background thread which is stopped when this is dropped.
pub struct DriverStation {
    shared: Arc<Shared>,
    commands: mpsc::UnboundedSender<Command>,
    thread: Option<JoinHandle<()>>,
}

impl DriverStation {
    pub fn builder() -> Builder {
        Builder {
            team_number: 0,
            alliance: Alliance::new_red(1),
            ip_override: None,
            ports: Ports::default(),
        }
    }

    /// Connects to the roboRIO for `team_number` at 10.TE.AM.2
    pub fn new_team(team_number: u32, alliance: Alliance) -> DriverStation {
        Self::builder()
            .team_number(team_number)
            .alliance(alliance)
            .connect()
    }

    fn retarget(&self) {
        let _ = self.commands.send(Command::Retarget);
    }

    /// Provides a closure called every 20ms to produce joystick values, one `Vec` per slot.
    ///
    /// The closure runs on the I/O thread and must not block.
    pub fn set_joystick_supplier(
        &self,
        supplier: impl Fn() -> Vec<Vec<JoystickValue>> + Send + Sync + 'static,
    ) {
        *self
            .shared
            .joystick_supplier
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(Box::new(supplier));
    }

    /// Provides a closure called for every TCP packet received from the roboRIO
    ///
    /// The closure runs on the I/O thread and must not block.
    pub fn set_tcp_consumer(&self, consumer: impl FnMut(TcpPacket) + Send + 'static) {
        *self
            .shared
            .tcp_consumer
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(Box::new(consumer));
    }

    pub fn set_alliance(&self, alliance: Alliance) {
        self.shared.control().alliance = alliance;
    }

    pub fn alliance(&self) -> Alliance {
        self.shared.control().alliance
    }

    pub fn set_mode(&self, mode: Mode) {
        self.shared.control().mode = mode;
    }

    pub fn mode(&self) -> Mode {
        self.shared.control().mode
    }

    pub fn ds_mode(&self) -> DsMode {
        self.shared.control().ds_mode
    }

    /// Changes the team number, and connects to that team's roboRIO unless USB is in use
    pub fn set_team_number(&self, team_number: u32) {
        let mut control = self.shared.control();
        if control.team_number != team_number {
            control.team_number = team_number;
            drop(control);
            self.retarget();
        }
    }

    pub fn team_number(&self) -> u32 {
        self.shared.control().team_number
    }

    /// Connects to the roboRIO's USB interface at 172.22.11.2 instead of the team address
    pub fn set_use_usb(&self, use_usb: bool) {
        let mut control = self.shared.control();
        if control.use_usb != use_usb {
            control.use_usb = use_usb;
            drop(control);
            self.retarget();
        }
    }

    pub fn use_usb(&self) -> bool {
        self.shared.control().use_usb
    }

    /// The address currently being used to reach the robot
    pub fn target_ip(&self) -> IpAddr {
        self.shared.control().target_ip()
    }

    /// Sets the Game Specific Message available to robot code. It is re-sent on every reconnect.
    pub fn set_game_specific_message(&self, message: &str) -> Result<()> {
        if message.len() > 3 || !message.is_ascii() {
            return Err(Error::InvalidGameData(message.to_string()));
        }
        self.shared.control().game_data = Some(message.to_string());
        let _ = self
            .commands
            .send(Command::SendTcp(TcpTag::GameData(message.to_string())));
        Ok(())
    }

    pub fn game_specific_message(&self) -> Option<String> {
        self.shared.control().game_data.clone()
    }

    /// Enables the robot. Ignored while e-stopped.
    pub fn enable(&self) {
        let mut control = self.shared.control();
        if !control.estopped {
            control.enabled = true;
        }
    }

    pub fn disable(&self) {
        self.shared.control().enabled = false;
    }

    pub fn enabled(&self) -> bool {
        self.shared.control().enabled
    }

    /// Disables the robot and keeps it disabled. The roboRIO refuses to enable until it reboots;
    /// the latch is released once communication drops (the reboot) or the target changes.
    pub fn estop(&self) {
        let mut control = self.shared.control();
        control.enabled = false;
        control.estopped = true;
    }

    pub fn estopped(&self) -> bool {
        self.shared.control().estopped
    }

    /// Stops sending the e-stop flag. If the robot still reports being e-stopped it is latched again.
    pub fn clear_estop(&self) {
        self.shared.control().estopped = false;
    }

    pub fn restart_code(&self) {
        self.shared.control().request |= Request::RESTART_CODE;
    }

    pub fn restart_roborio(&self) {
        self.shared.control().request |= Request::REBOOT_ROBORIO;
    }

    pub fn telemetry(&self) -> Telemetry {
        *self.shared.telemetry()
    }

    /// A problem preventing communication with any robot, such as the status port being taken
    /// by another driver station
    pub fn link_error(&self) -> Option<String> {
        self.shared
            .link_error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn is_connected(&self) -> bool {
        self.shared.telemetry().connected
    }

    pub fn is_code_running(&self) -> bool {
        self.shared.telemetry().code_running
    }

    pub fn trace(&self) -> Trace {
        self.shared.telemetry().trace
    }

    pub fn battery_voltage(&self) -> f32 {
        self.shared.telemetry().voltage
    }

    /// Queues a tag to be sent with the next control packet
    pub fn queue_udp(&self, tag: UdpTag) {
        self.shared.control().pending.push(tag);
    }

    pub(crate) fn apply_status(shared: &Shared, status: Status, trace: Trace, voltage: f32) {
        {
            let mut telemetry = shared.telemetry();
            telemetry.connected = true;
            telemetry.code_running = trace.is_code_started();
            telemetry.voltage = voltage;
            telemetry.brownout = status.is_browning_out();
            telemetry.trace = trace;
            telemetry.last_packet = Some(Instant::now());
        }
        if status.emergency_stopped() {
            let mut control = shared.control();
            control.estopped = true;
            control.enabled = false;
        }
    }
}

impl Drop for DriverStation {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn team_ips() {
        assert_eq!(ip_from_team_number(1).to_string(), "10.0.1.2");
        assert_eq!(ip_from_team_number(254).to_string(), "10.2.54.2");
        assert_eq!(ip_from_team_number(4069).to_string(), "10.40.69.2");
        assert_eq!(ip_from_team_number(10254).to_string(), "10.102.54.2");
    }

    #[test]
    fn joystick_layout() {
        let tag = joystick_tag(&[
            JoystickValue::Axis { id: 1, value: -1.0 },
            JoystickValue::Axis { id: 9, value: 1.0 },
            JoystickValue::Button {
                id: 1,
                pressed: true,
            },
            JoystickValue::Button {
                id: 0,
                pressed: true,
            },
            JoystickValue::POV { id: 0, angle: 90 },
        ]);
        assert_eq!(tag.axes, [0, -128, 0, 0, 0, 0]);
        assert!(tag.buttons[0]);
        assert_eq!(tag.buttons.iter().filter(|b| **b).count(), 1);
        assert_eq!(tag.povs, [90]);
        assert_eq!(joystick_tag(&[]), JoystickTag::default());
    }
}
