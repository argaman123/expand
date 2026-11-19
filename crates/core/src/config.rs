//! Per-machine settings, stored as TOML.

use crate::side::Side;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowMode {
    Windowed,
    Borderless,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamSettings {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_kbps: u32,
    pub window_mode: WindowMode,
}

impl Default for StreamSettings {
    fn default() -> Self {
        StreamSettings {
            width: 2560,
            height: 1440,
            fps: 60,
            bitrate_kbps: 30000,
            window_mode: WindowMode::Windowed,
        }
    }
}

/// The other machine, once trusted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Peer {
    pub name: String,
    pub id: String,
    pub host: String,
    pub token: String,
    #[serde(default)]
    pub sunshine_paired: bool,
    /// Every other address the Mac reported (its home network, Tailscale, a cable link). The PC
    /// tries them together with `host`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub addresses: Vec<String>,
}

impl Peer {
    /// Every address to try for this peer: the one typed in first, then the ones it reported.
    pub fn candidates(&self) -> Vec<std::net::IpAddr> {
        let mut out: Vec<std::net::IpAddr> = Vec::new();
        for text in std::iter::once(&self.host).chain(self.addresses.iter()) {
            if let Ok(ip) = text.trim().parse() {
                if !out.contains(&ip) {
                    out.push(ip);
                }
            }
        }
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    pub device_id: String,
    pub device_name: String,
    pub peer: Option<Peer>,
    pub stream: StreamSettings,
    pub sunshine_user: String,
    pub sunshine_pass: String,
    /// Share the mouse and keyboard with the peer whenever both apps are open.
    #[serde(default = "default_true")]
    pub mouse_sharing: bool,
    /// Which side of the PC the Mac sits on.
    #[serde(default = "default_mac_side")]
    pub mac_side: Side,
    /// The quick switch: all sharing (mouse, keyboard, clipboard) stopped until switched back.
    #[serde(default)]
    pub sharing_paused: bool,
    /// Share the clipboard (text, images, files) with the peer.
    #[serde(default = "default_true")]
    pub clipboard_sharing: bool,
    /// While the PC controls the Mac, the Mac types in the PC's keyboard language.
    #[serde(default = "default_true")]
    pub language_follow: bool,
    /// Set once the app has made its first-launch choices (starting at login).
    #[serde(default)]
    pub first_run_done: bool,
    /// Closing the window keeps the app running in the tray (or menu bar).
    #[serde(default = "default_true")]
    pub close_to_tray: bool,
}

fn default_true() -> bool {
    true
}

fn default_mac_side() -> Side {
    Side::Right
}

#[derive(Debug)]
pub enum ConfigError {
    Corrupt { path: PathBuf, reason: String },
    Io(std::io::Error),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Corrupt { path, reason } => {
                write!(f, "settings file {} is not valid: {reason}", path.display())
            }
            ConfigError::Io(e) => write!(f, "could not access settings file: {e}"),
        }
    }
}

impl std::error::Error for ConfigError {}

fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut buf);
    hex::encode(buf)
}

fn temporary_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".saving");
    path.with_file_name(name)
}

impl Config {
    pub fn default_new() -> Config {
        let device_name = std::env::var("COMPUTERNAME")
            .or_else(|_| std::env::var("HOSTNAME"))
            .unwrap_or_else(|_| "Sidescreen".to_string());
        Config {
            device_id: random_hex(16),
            device_name,
            peer: None,
            stream: StreamSettings::default(),
            sunshine_user: "sidescreen".to_string(),
            sunshine_pass: random_hex(16),
            mouse_sharing: true,
            mac_side: Side::Right,
            sharing_paused: false,
            close_to_tray: true,
            first_run_done: false,
            clipboard_sharing: true,
            language_follow: true,
        }
    }

    /// A missing file yields fresh defaults; an unreadable one is an error and is left as it is.
    pub fn load(path: &Path) -> Result<Config, ConfigError> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default_new()),
            Err(e) => return Err(ConfigError::Io(e)),
        };
        toml::from_str(&text).map_err(|e| ConfigError::Corrupt {
            path: path.to_path_buf(),
            reason: e.message().to_string(),
        })
    }

    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(ConfigError::Io)?;
        }
        let text = toml::to_string_pretty(self).expect("config is always serialisable");
        // Written beside the real file and swapped in, so a hard kill mid-save leaves either the
        // old settings or the new ones, never half of them.
        let temporary = temporary_path(path);
        std::fs::write(&temporary, text).map_err(ConfigError::Io)?;
        std::fs::rename(&temporary, path).map_err(ConfigError::Io)
    }

    pub fn default_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("sidescreen")
            .join("config.toml")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_peer() -> Peer {
        Peer {
            name: "MacBook".into(),
            id: "abc".into(),
            host: "192.168.1.50".into(),
            token: "t".repeat(64),
            sunshine_paired: true,
            addresses: Vec::new(),
        }
    }

    // A hard kill in the middle of saving must not leave a half-written settings file: that is
    // how the PC can lose its identity and its pairing (2026-11-18).
    #[test]
    fn saving_goes_through_a_temporary_file_that_is_then_swapped_in() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "old").unwrap();
        Config::default_new().save(&path).unwrap();
        assert!(Config::load(&path).is_ok());
        assert!(!temporary_path(&path).exists());
    }

    #[test]
    fn defaults_match_spec() {
        let c = Config::default_new();
        assert_eq!(
            (c.stream.width, c.stream.height, c.stream.fps, c.stream.bitrate_kbps),
            (2560, 1440, 60, 30000)
        );
        assert_eq!(c.stream.window_mode, WindowMode::Windowed);
        assert!(c.peer.is_none());
        assert_eq!(c.device_id.len(), 32);
        assert!(!c.sunshine_user.is_empty());
        assert!(c.sunshine_pass.len() >= 16);
    }

    #[test]
    fn mouse_sharing_defaults_on_with_the_mac_on_the_right() {
        let c = Config::default_new();
        assert!(c.mouse_sharing);
        assert_eq!(c.mac_side, crate::side::Side::Right);
    }

    // A settings file written before mouse sharing existed must still load.
    #[test]
    fn a_config_from_before_mouse_sharing_still_loads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"device_id = "0123456789abcdef0123456789abcdef"
device_name = "PC"
sunshine_user = "sidescreen"
sunshine_pass = "p"

[peer]
name = "Mac"
id = "m"
host = "192.168.1.50"
token = "t"
sunshine_paired = true

[stream]
width = 2560
height = 1440
fps = 60
bitrate_kbps = 30000
window_mode = "windowed"
"#,
        )
        .unwrap();
        let c = Config::load(&path).unwrap();
        assert!(c.mouse_sharing);
        assert_eq!(c.mac_side, crate::side::Side::Right);
        assert!(c.peer.is_some());
    }

    #[test]
    fn missing_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let c = Config::load(&dir.path().join("nope.toml")).unwrap();
        assert!(c.peer.is_none());
        assert_eq!(c.stream.width, 2560);
    }

    #[test]
    fn save_then_load_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("config.toml");
        let mut c = Config::default_new();
        c.peer = Some(sample_peer());
        c.stream.window_mode = WindowMode::Borderless;
        c.save(&path).unwrap();
        assert_eq!(Config::load(&path).unwrap(), c);
    }

    #[test]
    fn corrupt_file_is_an_error_and_left_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "not = [toml").unwrap();
        match Config::load(&path) {
            Err(ConfigError::Corrupt { path: p, .. }) => assert_eq!(p, path),
            other => panic!("expected Corrupt, got {other:?}"),
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "not = [toml");
    }
}
