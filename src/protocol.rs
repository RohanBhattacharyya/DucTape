//! Types shared by the web UI (over the WebSocket) and MCP clients.
//!
//! The backend owns all driver station state. Clients send [`ClientCommand`]s and receive a full
//! [`Snapshot`] whenever anything changes, so the UI and AI agents always agree on what the robot
//! is doing. The TypeScript mirror of these types lives in `ui/src/protocol.ts`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::console::ConsoleLine;

#[derive(Serialize, Deserialize, JsonSchema, Copy, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Autonomous,
    Teleoperated,
    Test,
}

impl Mode {
    pub fn to_ds(self) -> ds::Mode {
        match self {
            Mode::Autonomous => ds::Mode::Autonomous,
            Mode::Teleoperated => ds::Mode::Teleoperated,
            Mode::Test => ds::Mode::Test,
        }
    }

    pub fn from_ds(mode: ds::Mode) -> Mode {
        match mode {
            ds::Mode::Autonomous => Mode::Autonomous,
            ds::Mode::Teleoperated => Mode::Teleoperated,
            ds::Mode::Test => Mode::Test,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Mode::Autonomous => "Autonomous",
            Mode::Teleoperated => "Teleoperated",
            Mode::Test => "Test",
        }
    }
}

#[derive(Serialize, Deserialize, JsonSchema, Copy, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AllianceColor {
    Red,
    Blue,
}

#[derive(Serialize, Deserialize, JsonSchema, Copy, Clone, Debug, PartialEq, Eq)]
pub struct AllianceStation {
    pub color: AllianceColor,
    /// Driver station number, 1-3
    pub station: u8,
}

impl Default for AllianceStation {
    fn default() -> Self {
        AllianceStation {
            color: AllianceColor::Red,
            station: 1,
        }
    }
}

impl AllianceStation {
    pub fn to_ds(self) -> ds::Alliance {
        match self.color {
            AllianceColor::Red => ds::Alliance::new_red(self.station),
            AllianceColor::Blue => ds::Alliance::new_blue(self.station),
        }
    }

    pub fn from_ds(alliance: ds::Alliance) -> AllianceStation {
        AllianceStation {
            color: if alliance.is_red() {
                AllianceColor::Red
            } else {
                AllianceColor::Blue
            },
            station: alliance.position(),
        }
    }
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct JoystickDevice {
    /// Stable id for this device while it stays plugged in
    pub id: Uuid,
    pub name: String,
    /// True for Conductor's built-in virtual joystick, which MCP clients can drive
    #[serde(rename = "virtual")]
    pub is_virtual: bool,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct JoystickSlot {
    /// The port number robot code uses, 0-5
    pub slot: usize,
    pub device: Option<JoystickDevice>,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Default)]
pub struct VirtualJoystickState {
    /// Axes 0-5 in WPILib order: left X, left Y, left trigger, right trigger, right X, right Y
    pub axes: [f32; 6],
    /// Buttons 1-10 (index 0 is button 1): A, B, X, Y, LB, RB, Back, Start, L stick, R stick
    pub buttons: [bool; 10],
    /// POV angle in degrees, or -1 when released
    pub pov: i16,
}

/// The complete state of the driver station
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub team_number: u32,
    pub use_usb: bool,
    /// The address Conductor is sending control packets to
    pub target_address: String,
    pub mode: Mode,
    pub alliance: AllianceStation,
    /// Game Specific Message sent to robot code
    pub game_data: String,
    pub enabled: bool,
    pub estopped: bool,
    /// Communication with the robot (or simulator) is established
    pub comms: bool,
    /// Robot code is running
    pub code: bool,
    /// Connected to the WPILib simulator on this machine rather than a real robot
    pub simulator: bool,
    pub brownout: bool,
    pub voltage: f32,
    /// Summary shown in the UI, e.g. "Teleoperated Enabled" or "No Robot Code"
    pub status: String,
    /// A problem that prevents talking to any robot, e.g. another driver station is running
    pub link_error: Option<String>,
    pub joysticks: Vec<JoystickSlot>,
    pub unbound_joysticks: Vec<JoystickDevice>,
    pub virtual_joystick: VirtualJoystickState,
    /// Any physical controller is plugged in
    pub physical_joysticks: bool,
    /// Enter/Space are handled globally by the backend, regardless of window focus
    pub backend_keybinds: bool,
    /// If a timed enable is active, milliseconds until Conductor disables the robot
    pub auto_disable_in_ms: Option<u64>,
    /// Who last changed the enable state: "ui", "hotkey", "mcp" or "safety"
    pub last_enable_change_by: Option<String>,
}

/// Messages from the backend to the web UI
#[derive(Serialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    State(Box<Snapshot>),
    /// Replaces the client's console contents
    ConsoleBacklog {
        lines: Vec<ConsoleLine>,
    },
    Console {
        line: ConsoleLine,
    },
    ConsoleCleared,
    /// Static information about this Conductor instance
    Info(Info),
    /// A command from this client failed
    Error {
        message: String,
    },
}

#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone)]
pub struct Info {
    pub version: String,
    /// URL of the MCP Streamable HTTP endpoint, if enabled
    pub mcp_url: Option<String>,
    /// When MCP clients may enable the robot
    pub mcp_enable_policy: crate::config::EnablePolicy,
    pub mcp_max_enable_secs: u64,
}

/// Messages from the web UI to the backend
#[derive(Deserialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientCommand {
    SetTeamNumber {
        team_number: u32,
    },
    SetUseUsb {
        use_usb: bool,
    },
    SetGameData {
        game_data: String,
    },
    SetMode {
        mode: Mode,
    },
    SetAlliance {
        alliance: AllianceStation,
    },
    Enable,
    Disable,
    Estop,
    RestartCode,
    RebootRoborio,
    /// Reorder joystick slots, moving the device in `from` to `to`
    MoveJoystick {
        from: usize,
        to: usize,
    },
    BindJoystick {
        id: Uuid,
        slot: Option<usize>,
    },
    UnbindJoystick {
        id: Uuid,
    },
    ClearConsole,
}
