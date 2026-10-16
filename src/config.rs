//! Persistent settings, stored at `~/.config/conductor/conductor.toml` on Linux.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::protocol::AllianceStation;

const APP: &str = "conductor";

/// When MCP clients are allowed to enable the robot. Disabling and e-stopping are always allowed.
#[derive(
    Serialize, Deserialize, JsonSchema, clap::ValueEnum, Copy, Clone, Debug, PartialEq, Eq, Default,
)]
#[serde(rename_all = "lowercase")]
pub enum EnablePolicy {
    /// MCP clients can never enable the robot
    Never,
    /// Only while connected to the WPILib simulator on this machine
    #[default]
    Simulator,
    /// Also real robots. Make sure someone can reach the e-stop!
    Always,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct McpConfig {
    pub enable_policy: EnablePolicy,
    /// Upper bound on how long an MCP client can keep the robot enabled with one call
    pub max_enable_secs: u64,
    /// Serve MCP over Streamable HTTP on this address when the GUI runs, e.g. "127.0.0.1:7341"
    pub http_addr: Option<String>,
}

impl Default for McpConfig {
    fn default() -> Self {
        McpConfig {
            enable_policy: EnablePolicy::Simulator,
            max_enable_secs: 150,
            http_addr: None,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct Config {
    pub team_number: u32,
    pub use_usb: bool,
    pub alliance: AllianceStation,
    pub mcp: McpConfig,
}

pub fn load() -> Config {
    match confy::load(APP, APP) {
        Ok(cfg) => cfg,
        Err(e) => {
            tracing::warn!("could not load config, using defaults: {e}");
            Config::default()
        }
    }
}

pub fn store(cfg: &Config) {
    if let Err(e) = confy::store(APP, APP, cfg) {
        tracing::warn!("could not save config: {e}");
    }
}

pub fn path() -> Option<std::path::PathBuf> {
    confy::get_configuration_file_path(APP, APP).ok()
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
        let dir = std::env::temp_dir().join(format!("conductor-cfg-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("conductor.toml");
        std::fs::write(&path, s).unwrap();
        let cfg = confy::load_path(&path).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
        cfg
    }
}
