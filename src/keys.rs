//! Global hotkeys: Enter disables, Space e-stops, and `[` `]` `\` together enables,
//! regardless of which window has focus.

pub use self::arch::*;

#[cfg(target_os = "linux")]
#[path = "keys/linux.rs"]
mod arch;

#[cfg(target_os = "macos")]
#[path = "keys/mac.rs"]
mod arch;

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
#[path = "keys/default.rs"]
mod arch;
