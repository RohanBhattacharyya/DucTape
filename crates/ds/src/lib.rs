//! # ds
//!
//! `ds` allows for control of FIRST Robotics Competition robots. It speaks the driver station
//! protocol used by the roboRIO and the WPILib simulator, and exposes a small synchronous API.
//!
//! The core type is [`DriverStation`], which connects to a roboRIO and lets callers change its
//! mode and enable state, provide joystick input through an arbitrary callback, and consume the
//! console output and errors that robot code produces.

mod conn;
mod driver_station;
pub mod proto;
#[cfg(feature = "testing")]
pub mod testing;

pub use driver_station::{
    Builder, DriverStation, DsMode, JoystickValue, Mode, Ports, Telemetry, ip_from_team_number,
};
pub use proto::tcp::{ErrorMessage, Stdout, TcpPacket, TcpTag};
pub use proto::udp::{Alliance, Status, Trace};

/// Errors returned by the driver station API
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("game data must be at most 3 ASCII characters, got {0:?}")]
    InvalidGameData(String),
}

pub type Result<T> = std::result::Result<T, Error>;
