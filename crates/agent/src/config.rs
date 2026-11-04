//! The agent's settings in a file, so `run` needs no long command: `maker-connect-agent.toml`.
//!
//! ```toml
//! cloud = "http://192.168.1.20:8460"
//! name = "Dymo"
//! kind = "label"          # fdm, laser, label, ...
//! # port = "COM3"         # a printer or laser on a serial port
//! # baud = 115200
//! # firmware = "auto"    # auto, marlin, grbl
//! # state = "maker-connect-agent.json"
//! ```
//!
//! Found where `--config` says, else in the current folder, else next to the agent's own
//! program (so a double-click on Windows finds it). Options given on the command line win
//! over the file; `--save` writes them into it.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const FILE: &str = "maker-connect-agent.toml";

/// What a config file (or the command line) says; anything left out keeps its default.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cloud: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baud: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub firmware: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
}

impl Config {
    /// This config, with what `other` says on top.
    pub fn and(self, other: Config) -> Config {
        Config {
            cloud: other.cloud.or(self.cloud),
            name: other.name.or(self.name),
            kind: other.kind.or(self.kind),
            port: other.port.or(self.port),
            baud: other.baud.or(self.baud),
            firmware: other.firmware.or(self.firmware),
            state: other.state.or(self.state),
        }
    }
}

/// Where the config file is looked for: the current folder, then beside the program.
pub fn places() -> Vec<PathBuf> {
    let mut places = vec![PathBuf::from(FILE)];
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
    {
        places.push(dir.join(FILE));
    }
    places
}

/// The config file and what it says: the one named (it must be there), else the first
/// found. None when there is none.
pub fn load(named: Option<&str>) -> Result<Option<(PathBuf, Config)>, String> {
    let path = match named {
        Some(path) => PathBuf::from(path),
        None => match places().into_iter().find(|p| p.is_file()) {
            Some(path) => path,
            None => return Ok(None),
        },
    };
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("config {}: {e}", path.display()))?;
    let config = toml::from_str(&text).map_err(|e| format!("config {}: {e}", path.display()))?;
    Ok(Some((path, config)))
}

pub fn save(path: &Path, config: &Config) -> Result<(), String> {
    let text = format!(
        "# Maker Connect agent: `maker-connect-agent run` reads this. Options given on the\n\
         # command line win over it.\n{}",
        toml::to_string(config).map_err(|e| e.to_string())?
    );
    std::fs::write(path, text).map_err(|e| format!("config {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_is_read_and_the_command_line_wins() {
        let file: Config = toml::from_str(
            "cloud = \"http://192.168.1.20:8460\"\nname = \"Dymo\"\nkind = \"label\"\n",
        )
        .unwrap();
        let typed = Config {
            name: Some("Dymo at work".into()),
            ..Config::default()
        };
        let both = file.and(typed);
        assert_eq!(both.cloud.as_deref(), Some("http://192.168.1.20:8460"));
        assert_eq!(both.name.as_deref(), Some("Dymo at work"));
        assert_eq!(both.kind.as_deref(), Some("label"));
        assert!(
            toml::from_str::<Config>("clowd = \"x\"").is_err(),
            "a typo is said"
        );
        // Saved and read back the same.
        let text = toml::to_string(&both).unwrap();
        assert_eq!(toml::from_str::<Config>(&text).unwrap(), both);
    }
}
