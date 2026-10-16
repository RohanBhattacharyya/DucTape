//! Persistent settings, stored at `~/.config/ductape/ductape.toml` on Linux.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::protocol::AllianceStation;

const APP: &str = "ductape";

/// When MCP clients are allowed to enable the robot. Disabling and e-stopping are always allowed.
#[derive(
    Serialize, Deserialize, JsonSchema, clap::ValueEnum, Copy, Clone, Debug, PartialEq, Eq, Default,
)]
#[serde(rename_all = "lowercase")]
pub enum EnablePolicy {
    /// MCP clients can never enable the robot
    Never,
    /// Only while connected to the WPILib simulator on this machine
    Simulator,
    /// Real robots and the simulator (the default)
    #[default]
    Always,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct McpConfig {
    pub enable_policy: EnablePolicy,
    /// If set, MCP enables are timed and last at most this long. Unset means untimed
    pub max_enable_secs: Option<u64>,
    /// Serve MCP over Streamable HTTP on this address when the GUI runs, e.g. "127.0.0.1:7341"
    pub http_addr: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct Config {
    pub team_number: u32,
    pub use_usb: bool,
    pub alliance: AllianceStation,
    pub mcp: McpConfig,
}

/// The config file: `$DUCTAPE_CONFIG` (or `--config`), else the platform default
pub fn path() -> Option<PathBuf> {
    match std::env::var_os("DUCTAPE_CONFIG") {
        Some(path) => Some(path.into()),
        None => confy::get_configuration_file_path(APP, APP).ok(),
    }
}

pub fn load() -> Config {
    let Some(path) = path() else {
        return Config::default();
    };
    // Carry settings over from Conductor, which DucTape revives
    if !path.exists()
        && std::env::var_os("DUCTAPE_CONFIG").is_none()
        && let Ok(legacy) = confy::get_configuration_file_path("conductor", "conductor")
        && legacy.exists()
        && let Ok(cfg) = confy::load_path::<Config>(&legacy)
    {
        tracing::info!("importing settings from {}", legacy.display());
        store(&cfg);
        return cfg;
    }
    match confy::load_path(&path) {
        Ok(cfg) => cfg,
        Err(e) => {
            tracing::warn!("could not load {}, using defaults: {e}", path.display());
            Config::default()
        }
    }
}

pub fn store(cfg: &Config) {
    let Some(path) = path() else { return };
    if let Err(e) = confy::store_path(&path, cfg) {
        tracing::warn!("could not save {}: {e}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_legacy_config() {
        // Conductor 0.1 only stored the team number
        let cfg: Config = toml_from_str("team_number = 4069\n");
        assert_eq!(cfg.team_number, 4069);
        assert_eq!(cfg.mcp, McpConfig::default());
    }

    fn toml_from_str(s: &str) -> Config {
        let dir = std::env::temp_dir().join(format!("ductape-cfg-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ductape.toml");
        std::fs::write(&path, s).unwrap();
        let cfg = confy::load_path(&path).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
        cfg
    }
}
