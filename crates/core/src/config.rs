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
    /// The monitor number the viewer opens on; `None` means "toward the Mac".
    #[serde(default)]
    pub monitor: Option<u32>,
}

impl Default for StreamSettings {
    fn default() -> Self {
        StreamSettings {
            width: 2560,
            height: 1440,
            fps: 60,
            bitrate_kbps: 30000,
            window_mode: WindowMode::Windowed,
            monitor: None,
        }
    }
}

/// Where a peer sits relative to this PC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Placement {
    pub side: Side,
    /// Used only when `side` is `Bottom`: the monitor number the peer sits below.
    #[serde(default)]
    pub monitor: Option<u32>,
}

impl Default for Placement {
    fn default() -> Self {
        Placement { side: Side::Right, monitor: None }
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
    #[serde(default)]
    pub placement: Placement,
    #[serde(default)]
    pub stream: StreamSettings,
    #[serde(default = "default_true")]
    pub allowed: bool,
}

impl Peer {
    /// What identifies this peer: its id, or (before the id is known) its address.
    pub fn key(&self) -> String {
        if self.id.is_empty() {
            format!("host:{}", self.host)
        } else {
            self.id.clone()
        }
    }

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
    #[serde(default)]
    pub peers: Vec<Peer>,
    /// The key of the peer the app is working with.
    #[serde(default)]
    pub current: Option<String>,
    /// Connect to the current peer whenever it is reachable.
    #[serde(default = "default_true")]
    pub auto_connect: bool,
    /// Keep this machine awake while connected.
    #[serde(default = "default_true")]
    pub stay_awake: bool,
    pub sunshine_user: String,
    pub sunshine_pass: String,
    /// Share the mouse and keyboard with the peer whenever both apps are open.
    #[serde(default = "default_true")]
    pub mouse_sharing: bool,
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
    /// PC: the display starts as soon as the connection to the Mac comes up.
    #[serde(default)]
    pub start_display_on_connect: bool,
    /// PC: while the Mac has the keyboard, Ctrl reaches it as Command and Windows as Control.
    #[serde(default = "default_true")]
    pub ctrl_as_cmd: bool,
}

fn default_true() -> bool {
    true
}

/// Which side of the pair this machine is, for reading an old settings file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Windows: its peers are Macs.
    Pc,
    /// macOS: its peers are PCs.
    Mac,
}

/// The fields of the single-peer settings files, read only to migrate them.
#[derive(Deserialize)]
struct Legacy {
    peer: Option<Peer>,
    mac_side: Option<Side>,
    stream: Option<StreamSettings>,
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

/// Where a settings file in the old single-peer shape is kept when it is migrated.
pub fn backup_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".v1");
    path.with_file_name(name)
}

fn temporary_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".saving");
    path.with_file_name(name)
}

/// What a machine is called when its computer name cannot be read.
const FALLBACK_NAME: &str = "Sidescreen";

/// The name the owner gave this computer: on the Mac its Sharing name (an app opened from the
/// Dock has no `HOSTNAME`), on Windows `COMPUTERNAME`.
pub fn computer_name() -> Option<String> {
    #[cfg(target_os = "macos")]
    let name = std::process::Command::new("/usr/sbin/scutil")
        .args(["--get", "ComputerName"])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned());
    #[cfg(windows)]
    let name = std::env::var("COMPUTERNAME").ok();
    #[cfg(not(any(windows, target_os = "macos")))]
    let name = std::env::var("HOSTNAME").ok();
    name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty())
}

/// The name to use instead of `current` when that is empty or the old fallback, if the computer
/// has a better one. `None` keeps `current`.
fn named_after_computer(current: &str, computer: impl FnOnce() -> Option<String>) -> Option<String> {
    if !current.trim().is_empty() && current != FALLBACK_NAME {
        return None;
    }
    computer().filter(|name| name != current && name != FALLBACK_NAME)
}

impl Config {
    pub fn default_new() -> Config {
        let device_name = computer_name().unwrap_or_else(|| FALLBACK_NAME.to_string());
        Config {
            device_id: random_hex(16),
            device_name,
            peers: Vec::new(),
            current: None,
            auto_connect: true,
            stay_awake: true,
            sunshine_user: "sidescreen".to_string(),
            sunshine_pass: random_hex(16),
            mouse_sharing: true,
            close_to_tray: true,
            first_run_done: false,
            clipboard_sharing: true,
            language_follow: true,
            start_display_on_connect: false,
            ctrl_as_cmd: true,
        }
    }

    /// [`Config::load_as`] for the PC.
    pub fn load(path: &Path) -> Result<Config, ConfigError> {
        Config::load_as(path, Role::Pc)
    }

    /// A missing file yields fresh defaults; an unreadable one is an error and is left as it is.
    /// An old single-peer file is migrated; on the Mac its PC keeps its pairing but not its id,
    /// which 1.0 never checked (so it may be stale) and which is learnt again when the PC next
    /// proves itself. A Mac still called by the old fallback name takes its computer name.
    pub fn load_as(path: &Path, role: Role) -> Result<Config, ConfigError> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default_new()),
            Err(e) => return Err(ConfigError::Io(e)),
        };
        let corrupt = |e: toml::de::Error| ConfigError::Corrupt {
            path: path.to_path_buf(),
            reason: e.message().to_string(),
        };
        let mut config: Config = toml::from_str(&text).map_err(corrupt)?;
        let legacy: Legacy = toml::from_str(&text).map_err(corrupt)?;
        if config.peers.is_empty() {
            if let Some(mut peer) = legacy.peer {
                // The migrated file is saved in the new shape straight after: keep the old one so
                // rolling back to an older Sidescreen still finds its pairing.
                let backup = backup_path(path);
                if !backup.exists() {
                    std::fs::copy(path, &backup).map_err(ConfigError::Io)?;
                }
                peer.placement = Placement { side: legacy.mac_side.unwrap_or(Side::Right), monitor: None };
                peer.stream = legacy.stream.unwrap_or_default();
                if role == Role::Mac {
                    peer.id.clear();
                }
                config.current = Some(peer.key());
                config.peers.push(peer);
            }
        }
        // A Mac that could not read its own name before called itself "Sidescreen". The caller
        // saves the loaded settings straight away, which keeps the new name.
        if role == Role::Mac {
            if let Some(name) = named_after_computer(&config.device_name, computer_name) {
                config.device_name = name;
            }
        }
        Ok(config)
    }

    pub fn peer(&self, key: &str) -> Option<&Peer> {
        self.peers.iter().find(|p| p.key() == key)
    }

    pub fn peer_mut(&mut self, key: &str) -> Option<&mut Peer> {
        self.peers.iter_mut().find(|p| p.key() == key)
    }

    pub fn current_peer(&self) -> Option<&Peer> {
        self.peer(self.current.as_deref()?)
    }

    pub fn current_peer_mut(&mut self) -> Option<&mut Peer> {
        let key = self.current.clone()?;
        self.peer_mut(&key)
    }

    /// Adds the peer or replaces the one with the same key. A peer saved by address alone is
    /// also replaced when the same address now arrives with an id (the Mac's id is learnt on the
    /// first connection). Returns the key.
    pub fn upsert_peer(&mut self, p: Peer) -> String {
        let key = p.key();
        let at = self
            .peers
            .iter()
            .position(|e| e.key() == key)
            .or_else(|| {
                (!p.id.is_empty())
                    .then(|| self.peers.iter().position(|e| e.id.is_empty() && e.host == p.host))
                    .flatten()
            });
        match at {
            Some(i) => {
                let old = self.peers[i].key();
                self.peers[i] = p;
                if old != key && self.current.as_deref() == Some(old.as_str()) {
                    self.current = Some(key.clone());
                }
            }
            None => self.peers.push(p),
        }
        key
    }

    pub fn remove_peer(&mut self, key: &str) -> bool {
        let before = self.peers.len();
        self.peers.retain(|p| p.key() != key);
        if self.current.as_deref() == Some(key) {
            self.current = None;
        }
        self.peers.len() != before
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

    /// `SIDESCREEN_CONFIG`, when set and not empty, replaces the default location (for test runs).
    pub fn default_path() -> PathBuf {
        if let Some(path) = std::env::var_os("SIDESCREEN_CONFIG").filter(|p| !p.is_empty()) {
            return PathBuf::from(path);
        }
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
            placement: Placement::default(),
            stream: StreamSettings::default(),
            allowed: true,
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
        let s = StreamSettings::default();
        assert_eq!((s.width, s.height, s.fps, s.bitrate_kbps), (2560, 1440, 60, 30000));
        assert_eq!(s.window_mode, WindowMode::Windowed);
        assert_eq!(s.monitor, None);
        assert!(c.peers.is_empty());
        assert_eq!(c.device_id.len(), 32);
        assert!(!c.sunshine_user.is_empty());
        assert!(c.sunshine_pass.len() >= 16);
    }

    #[test]
    fn mouse_sharing_defaults_on_with_the_mac_on_the_right() {
        let c = Config::default_new();
        assert!(c.mouse_sharing);
        assert_eq!(Placement::default().side, Side::Right);
    }

    #[test]
    fn new_configs_have_no_peers_and_connect_automatically() {
        let c = Config::default_new();
        assert!(c.peers.is_empty());
        assert!(c.current.is_none());
        assert!(c.auto_connect);
        assert!(c.stay_awake);
    }

    const OLD_FILE: &str = r#"device_id = "0123456789abcdef0123456789abcdef"
device_name = "Windows PC"
sunshine_user = "sidescreen"
sunshine_pass = "p"
mac_side = "left"
sharing_paused = true

[peer]
name = "MacBook"
id = "m"
host = "192.168.1.20"
token = "t"
sunshine_paired = true

[stream]
width = 1920
height = 1080
fps = 60
bitrate_kbps = 30000
window_mode = "windowed"
"#;

    #[test]
    fn an_old_single_peer_file_migrates_into_peers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, OLD_FILE).unwrap();
        let c = Config::load(&path).unwrap();
        assert_eq!(c.peers.len(), 1);
        assert_eq!(c.peers[0].placement.side, Side::Left);
        assert_eq!(c.peers[0].stream.width, 1920);
        assert!(c.peers[0].allowed);
        assert_eq!(c.current, Some(c.peers[0].key()));
        assert!(c.auto_connect);
    }

    #[test]
    fn a_file_from_before_the_display_and_keyboard_options_gets_their_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, OLD_FILE).unwrap();
        let c = Config::load(&path).unwrap();
        assert!(!c.start_display_on_connect);
        assert!(c.ctrl_as_cmd);
        let new = Config::default_new();
        assert!(!new.start_display_on_connect && new.ctrl_as_cmd);
    }

    // Live check, task 9: the id a 1.0 Mac kept for its PC can be stale (1.0 never checked it),
    // so the Mac's migrated PC keeps its token but not its id, which is learnt again.
    #[test]
    fn a_macs_old_file_migrates_its_pc_without_trusting_the_pcs_id() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, OLD_FILE).unwrap();
        let c = Config::load_as(&path, Role::Mac).unwrap();
        assert_eq!(c.peers.len(), 1);
        assert_eq!(c.peers[0].id, "", "the stale id is dropped");
        assert_eq!(c.peers[0].token, "t", "the pairing is kept");
        assert_eq!((c.peers[0].name.as_str(), c.peers[0].host.as_str()), ("MacBook", "192.168.1.20"));
        assert_eq!(c.current, Some("host:192.168.1.20".to_string()));
        assert!(c.current_peer().is_some());
        // The PC keeps the Mac's id as it was.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, OLD_FILE).unwrap();
        assert_eq!(Config::load(&path).unwrap().peers[0].id, "m");
        assert_eq!(Config::load_as(&path, Role::Pc).unwrap().peers[0].id, "m");
    }

    // Final review I8: rolling back to the single-peer app must still find the pairing.
    #[test]
    fn migrating_keeps_the_old_file_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, OLD_FILE).unwrap();
        let c = Config::load(&path).unwrap();
        assert_eq!(std::fs::read_to_string(backup_path(&path)).unwrap(), OLD_FILE);
        assert_eq!(backup_path(&path), dir.path().join("config.toml.v1"));
        // A backup that is there is never overwritten.
        std::fs::write(backup_path(&path), "first").unwrap();
        Config::load(&path).unwrap();
        assert_eq!(std::fs::read_to_string(backup_path(&path)).unwrap(), "first");
        // Nothing to migrate: no backup.
        std::fs::remove_file(backup_path(&path)).unwrap();
        c.save(&path).unwrap();
        Config::load(&path).unwrap();
        assert!(!backup_path(&path).exists());
    }

    #[test]
    fn learning_a_macs_id_updates_the_migrated_entry() {
        let mut c = Config::default_new();
        let mut p = sample_peer();
        p.id = String::new();
        p.host = "192.168.1.20".into();
        let key = c.upsert_peer(p.clone());
        assert_eq!(key, "host:192.168.1.20");
        c.current = Some(key);
        p.id = "abc".into();
        assert_eq!(c.upsert_peer(p), "abc");
        assert_eq!(c.peers.len(), 1);
        assert_eq!(c.current, Some("abc".to_string()));
    }

    #[test]
    fn removing_the_current_peer_clears_current() {
        let mut c = Config::default_new();
        let key = c.upsert_peer(sample_peer());
        c.current = Some(key.clone());
        assert!(c.remove_peer(&key));
        assert!(c.peers.is_empty());
        assert!(c.current.is_none());
        assert!(!c.remove_peer(&key));
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
        assert_eq!(c.peers[0].placement.side, Side::Right);
        assert!(c.current_peer().is_some());
    }

    #[test]
    fn missing_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let c = Config::load(&dir.path().join("nope.toml")).unwrap();
        assert!(c.peers.is_empty());
    }

    #[test]
    fn save_then_load_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("config.toml");
        let mut c = Config::default_new();
        let mut p = sample_peer();
        p.stream.window_mode = WindowMode::Borderless;
        p.placement = Placement { side: Side::Bottom, monitor: Some(2) };
        c.current = Some(c.upsert_peer(p));
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

    // Live check, task 9: a Mac app started from the Dock has neither COMPUTERNAME nor HOSTNAME,
    // so it called itself "Sidescreen" and the PC showed that name.
    #[test]
    fn a_missing_or_fallback_name_takes_the_computer_name() {
        let mac = "Argaman\u{2019}s MacBook Pro";
        let computer = || Some(mac.to_string());
        assert_eq!(named_after_computer("Sidescreen", computer).as_deref(), Some(mac));
        assert_eq!(named_after_computer("", computer).as_deref(), Some(mac));
        assert_eq!(named_after_computer("  ", computer).as_deref(), Some(mac));
    }

    #[test]
    fn any_other_name_is_kept() {
        let computer = || Some("Studio".to_string());
        assert_eq!(named_after_computer("Work Mac", computer), None);
        assert_eq!(named_after_computer("sidescreen", computer), None);
        // Nothing better to offer: left as it is.
        assert_eq!(named_after_computer("Sidescreen", || None), None);
        assert_eq!(named_after_computer("Sidescreen", || Some("Sidescreen".to_string())), None);
    }

    #[test]
    fn the_computer_name_is_never_blank() {
        if let Some(name) = computer_name() {
            assert!(!name.trim().is_empty());
            assert_eq!(name, name.trim());
        }
    }

    #[test]
    fn loading_names_a_mac_after_its_computer_but_leaves_the_pc_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut c = Config::default_new();
        c.device_name = "Sidescreen".into();
        c.save(&path).unwrap();
        let expected = computer_name().unwrap_or_else(|| "Sidescreen".to_string());
        assert_eq!(Config::load_as(&path, Role::Mac).unwrap().device_name, expected);
        assert_eq!(Config::load_as(&path, Role::Pc).unwrap().device_name, "Sidescreen");
        c.device_name = "Work Mac".into();
        c.save(&path).unwrap();
        assert_eq!(Config::load_as(&path, Role::Mac).unwrap().device_name, "Work Mac");
    }

    #[test]
    fn the_config_path_can_be_overridden_for_testing() {
        // The only test that touches this variable, so no other test can race it.
        std::env::set_var("SIDESCREEN_CONFIG", "/tmp/sidescreen-test/config.toml");
        assert_eq!(Config::default_path(), PathBuf::from("/tmp/sidescreen-test/config.toml"));
        std::env::set_var("SIDESCREEN_CONFIG", "");
        assert!(Config::default_path().ends_with("sidescreen/config.toml"));
        std::env::remove_var("SIDESCREEN_CONFIG");
    }
}
