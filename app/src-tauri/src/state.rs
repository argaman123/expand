//! What the app remembers while running, kept free of window code so it can be tested.

use serde::Serialize;
use sidescreen_core::config::{Config, Peer, StreamSettings, WindowMode};
use sidescreen_core::side::Side;
use sidescreen_core::mouse::{MouseRole, MouseStatus};
use sidescreen_core::session::State;
use std::path::PathBuf;
use std::sync::Mutex;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Serialize)]
pub struct SettingsDto {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_mbps: u32,
    pub window_mode: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct StatusDto {
    /// "client" on the PC, "host" on the Mac.
    pub role: &'static str,
    /// "disconnected", "connecting", "connected" or "failed".
    pub state: &'static str,
    pub part: Option<String>,
    pub reason: Option<String>,
    pub peer_name: Option<String>,
    pub peer_host: Option<String>,
    pub paired: bool,
    pub settings: SettingsDto,
    pub config_error: Option<String>,
    /// Whether the user wants mouse and keyboard sharing.
    pub mouse_sharing: bool,
    /// "left", "right", "top" or "bottom": where the Mac sits relative to the PC.
    pub mac_side: &'static str,
    /// "off", "running" or "failed".
    pub mouse_state: &'static str,
    pub mouse_reason: Option<String>,
    /// The quick switch that stops all sharing.
    pub sharing_paused: bool,
    /// Closing the window keeps the app running in the background.
    pub close_to_tray: bool,
    pub clipboard_sharing: bool,
    pub language_follow: bool,
    /// Other addresses the Mac reported (Tailscale, a cable link, another network).
    pub peer_addresses: Vec<String>,
}

/// The tray's tooltip while sharing is paused.
pub fn tray_text_paused(state: &str) -> String {
    format!("Sidescreen: {state} · sharing paused")
}

/// The tray's tooltip: the display's state, and mouse sharing whenever it is on.
pub fn tray_text(state: &str, mouse_state: &str) -> String {
    match mouse_state {
        "running" => format!("Sidescreen: {state} · mouse sharing on"),
        "failed" => format!("Sidescreen: {state} · mouse sharing not working"),
        _ => format!("Sidescreen: {state}"),
    }
}

pub const ROLE: &str = if cfg!(target_os = "macos") { "host" } else { "client" };

pub struct AppState {
    pub cfg: Mutex<Config>,
    pub path: PathBuf,
    pub state: watch::Sender<State>,
    active: Mutex<Option<CancellationToken>>,
    config_error: Mutex<Option<String>>,
    /// The mouse-sharing engine's last reported status.
    mouse_status: Mutex<MouseStatus>,
}

impl AppState {
    pub fn new(cfg: Config, path: PathBuf, config_error: Option<String>) -> AppState {
        AppState {
            cfg: Mutex::new(cfg),
            path,
            state: watch::channel(State::Disconnected).0,
            active: Mutex::new(None),
            config_error: Mutex::new(config_error),
            mouse_status: Mutex::new(MouseStatus::Off),
        }
    }

    /// Claims the single session slot. `None` when a session is already running.
    pub fn begin_session(&self) -> Option<CancellationToken> {
        let mut active = self.active.lock().unwrap();
        if active.is_some() {
            return None;
        }
        let token = CancellationToken::new();
        *active = Some(token.clone());
        Some(token)
    }

    pub fn end_session(&self) {
        *self.active.lock().unwrap() = None;
    }

    pub fn has_session(&self) -> bool {
        self.active.lock().unwrap().is_some()
    }

    /// Asks the running session to stop. False when there is none.
    pub fn cancel_session(&self) -> bool {
        match self.active.lock().unwrap().as_ref() {
            Some(token) => {
                token.cancel();
                true
            }
            None => false,
        }
    }

    pub fn is_idle(&self) -> bool {
        matches!(*self.state.borrow(), State::Disconnected | State::Failed { .. })
    }

    fn update(&self, change: impl FnOnce(&mut Config)) -> Result<(), String> {
        if self.config_error.lock().unwrap().is_some() {
            return Err("the settings file is damaged; reset it first".to_string());
        }
        let mut cfg = self.cfg.lock().unwrap();
        change(&mut cfg);
        cfg.save(&self.path).map_err(|e| e.to_string())
    }

    pub fn save_settings(&self, s: StreamSettings) -> Result<(), String> {
        if !self.is_idle() {
            return Err("disconnect before changing settings".to_string());
        }
        let ok = (640..=7680).contains(&s.width)
            && (360..=4320).contains(&s.height)
            && (10..=240).contains(&s.fps)
            && (1_000..=150_000).contains(&s.bitrate_kbps);
        if !ok {
            return Err("those settings are out of range".to_string());
        }
        self.update(|cfg| cfg.stream = s)
    }

    /// Points the app at a Mac by address. It is not trusted until the first connect pairs.
    pub fn set_peer_host(&self, host: &str) -> Result<(), String> {
        let host = host.trim();
        if host.parse::<std::net::IpAddr>().is_err() {
            return Err("enter the Mac's IP address, for example 192.168.1.50".to_string());
        }
        self.update(|cfg| {
            cfg.peer = Some(Peer {
                name: host.to_string(),
                id: String::new(),
                host: host.to_string(),
                token: String::new(),
                sunshine_paired: false,
                addresses: Vec::new(),
            })
        })
    }

    pub fn set_peer(&self, peer: Peer) -> Result<(), String> {
        self.update(|cfg| cfg.peer = Some(peer))
    }

    pub fn set_mouse_sharing(&self, enabled: bool) -> Result<(), String> {
        self.update(|cfg| cfg.mouse_sharing = enabled)
    }

    pub fn set_clipboard_sharing(&self, enabled: bool) -> Result<(), String> {
        self.update(|cfg| cfg.clipboard_sharing = enabled)
    }

    pub fn set_language_follow(&self, enabled: bool) -> Result<(), String> {
        self.update(|cfg| cfg.language_follow = enabled)
    }

    /// True the first time it is asked, then never again.
    pub fn take_first_run(&self) -> bool {
        let first = !self.cfg.lock().unwrap().first_run_done;
        if first {
            let _ = self.update(|cfg| cfg.first_run_done = true);
        }
        first
    }

    pub fn set_close_to_tray(&self, enabled: bool) -> Result<(), String> {
        self.update(|cfg| cfg.close_to_tray = enabled)
    }

    pub fn set_sharing_paused(&self, paused: bool) -> Result<(), String> {
        self.update(|cfg| cfg.sharing_paused = paused)
    }

    pub fn set_mac_side(&self, side: Side) -> Result<(), String> {
        self.update(|cfg| cfg.mac_side = side)
    }

    pub fn set_mouse_status(&self, status: MouseStatus) {
        *self.mouse_status.lock().unwrap() = status;
    }

    /// What mouse sharing should run as, or `None` when it should be off: switched off, or
    /// not paired yet.
    pub fn mouse_role(&self) -> Option<MouseRole> {
        let cfg = self.cfg.lock().unwrap();
        if cfg.sharing_paused || !(cfg.mouse_sharing || cfg.clipboard_sharing) {
            return None;
        }
        let peer = cfg.peer.as_ref().filter(|p| !p.token.is_empty())?;
        #[cfg(windows)]
        return Some(MouseRole::Pc(sidescreen_core::share::pc::PcSettings {
            macs: Some(peer.candidates()).filter(|c| !c.is_empty())?,
            my_id: cfg.device_id.clone(),
            token: peer.token.clone(),
            mac_side: cfg.mac_side,
            stream: (cfg.stream.width, cfg.stream.height),
            mouse: cfg.mouse_sharing,
            clipboard: cfg.clipboard_sharing,
            language: cfg.language_follow,
        }));
        #[cfg(target_os = "macos")]
        return Some(MouseRole::Mac(sidescreen_core::share::mac::MacSettings {
            pc_id: peer.id.clone(),
            token: peer.token.clone(),
            clipboard: cfg.clipboard_sharing,
        }));
        #[allow(unreachable_code)]
        None
    }

    pub fn forget_peer(&self) -> Result<(), String> {
        if !self.is_idle() {
            return Err("disconnect first".to_string());
        }
        self.update(|cfg| cfg.peer = None)
    }

    /// Replaces a damaged settings file with fresh defaults.
    pub fn reset_config(&self) -> Result<(), String> {
        *self.config_error.lock().unwrap() = None;
        self.update(|cfg| *cfg = Config::default_new())
    }

    pub fn status(&self) -> StatusDto {
        let mouse = self.mouse_status.lock().unwrap().clone();
        let cfg = self.cfg.lock().unwrap();
        let (state, part, reason) = match &*self.state.borrow() {
            State::Disconnected => ("disconnected", None, None),
            State::Connecting => ("connecting", None, None),
            State::Connected => ("connected", None, None),
            State::Failed { part, reason } => ("failed", Some(part.clone()), Some(reason.clone())),
        };
        StatusDto {
            role: ROLE,
            state,
            part,
            reason,
            peer_name: cfg.peer.as_ref().map(|p| p.name.clone()),
            peer_host: cfg.peer.as_ref().map(|p| p.host.clone()),
            paired: cfg.peer.as_ref().is_some_and(|p| !p.token.is_empty()),
            settings: SettingsDto {
                width: cfg.stream.width,
                height: cfg.stream.height,
                fps: cfg.stream.fps,
                bitrate_mbps: cfg.stream.bitrate_kbps / 1000,
                window_mode: match cfg.stream.window_mode {
                    WindowMode::Windowed => "windowed".to_string(),
                    WindowMode::Borderless => "borderless".to_string(),
                },
            },
            config_error: self.config_error.lock().unwrap().clone(),
            mouse_sharing: cfg.mouse_sharing,
            mac_side: match cfg.mac_side {
                Side::Left => "left",
                Side::Right => "right",
                Side::Top => "top",
                Side::Bottom => "bottom",
            },
            mouse_state: match &mouse {
                MouseStatus::Off => "off",
                MouseStatus::Running => "running",
                MouseStatus::Failed(_) => "failed",
            },
            mouse_reason: match mouse {
                MouseStatus::Failed(reason) => Some(reason),
                _ => None,
            },
            sharing_paused: cfg.sharing_paused,
            close_to_tray: cfg.close_to_tray,
            clipboard_sharing: cfg.clipboard_sharing,
            language_follow: cfg.language_follow,
            peer_addresses: cfg.peer.as_ref().map(|p| p.addresses.clone()).unwrap_or_default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sidescreen_core::config::{Config, StreamSettings, WindowMode};
    #[allow(unused_imports)] // only the PC's tests choose a side
    use sidescreen_core::side::Side;
    use sidescreen_core::mouse::MouseStatus;
    use sidescreen_core::session::State;

    fn app() -> (AppState, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        (AppState::new(Config::default_new(), path, None), dir)
    }

    fn settings(width: u32, fps: u32, bitrate_kbps: u32) -> StreamSettings {
        StreamSettings { width, height: 1080, fps, bitrate_kbps, window_mode: WindowMode::Borderless }
    }

    #[test]
    fn connect_is_ignored_while_a_session_is_active() {
        let (a, _d) = app();
        assert!(a.begin_session().is_some());
        assert!(a.begin_session().is_none());
    }

    #[test]
    fn a_new_session_can_start_after_the_last_one_ended() {
        let (a, _d) = app();
        a.begin_session().unwrap();
        a.end_session();
        assert!(a.begin_session().is_some());
    }

    #[test]
    fn cancel_session_cancels_the_active_token() {
        let (a, _d) = app();
        assert!(!a.cancel_session());
        let token = a.begin_session().unwrap();
        assert!(a.cancel_session());
        assert!(token.is_cancelled());
    }

    #[test]
    fn save_settings_is_rejected_while_connected() {
        let (a, _d) = app();
        a.state.send_replace(State::Connected);
        assert!(a.save_settings(settings(1920, 60, 20000)).is_err());
        a.state.send_replace(State::Connecting);
        assert!(a.save_settings(settings(1920, 60, 20000)).is_err());
    }

    #[test]
    fn save_settings_persists_when_idle() {
        let (a, _d) = app();
        a.save_settings(settings(1920, 60, 20000)).unwrap();
        assert_eq!(Config::load(&a.path).unwrap().stream, settings(1920, 60, 20000));
        a.state.send_replace(State::Failed { part: "x".into(), reason: "y".into() });
        a.save_settings(settings(2560, 120, 50000)).unwrap();
    }

    #[test]
    fn save_settings_rejects_values_out_of_range() {
        let (a, _d) = app();
        assert!(a.save_settings(settings(100, 60, 20000)).is_err());
        assert!(a.save_settings(settings(1920, 0, 20000)).is_err());
        assert!(a.save_settings(settings(1920, 60, 500)).is_err());
        assert!(a.save_settings(settings(1920, 60, 500_000)).is_err());
    }

    #[test]
    fn set_peer_host_needs_an_ip_address_and_starts_unpaired() {
        let (a, _d) = app();
        assert!(a.set_peer_host("not an address").is_err());
        a.set_peer_host(" 192.168.1.50 ").unwrap();
        let peer = Config::load(&a.path).unwrap().peer.unwrap();
        assert_eq!(peer.host, "192.168.1.50");
        assert!(peer.token.is_empty());
        assert!(!peer.sunshine_paired);
    }

    #[test]
    fn forget_peer_clears_it() {
        let (a, _d) = app();
        a.set_peer_host("192.168.1.50").unwrap();
        a.forget_peer().unwrap();
        assert!(Config::load(&a.path).unwrap().peer.is_none());
    }

    #[test]
    fn status_reports_failure_details_and_settings() {
        let (a, _d) = app();
        a.set_peer_host("192.168.1.50").unwrap();
        a.state.send_replace(State::Failed { part: "Sunshine".into(), reason: "already running".into() });
        let s = a.status();
        assert_eq!(s.state, "failed");
        assert_eq!(s.part.as_deref(), Some("Sunshine"));
        assert_eq!(s.reason.as_deref(), Some("already running"));
        assert_eq!(s.peer_host.as_deref(), Some("192.168.1.50"));
        assert!(!s.paired);
        assert_eq!((s.settings.width, s.settings.bitrate_mbps), (2560, 30));
        assert_eq!(s.settings.window_mode, "windowed");
    }

    fn paired(a: &AppState) {
        a.set_peer(Peer {
            name: "MacBook".into(),
            id: "macid".into(),
            host: "192.168.1.50".into(),
            token: "t".repeat(64),
            sunshine_paired: true,
            addresses: Vec::new(),
        })
        .unwrap();
    }

    #[test]
    fn no_mouse_sharing_until_paired() {
        let (a, _d) = app();
        assert!(a.mouse_role().is_none());
        a.set_peer_host("192.168.1.50").unwrap();
        assert!(a.mouse_role().is_none(), "an address alone is not a pairing");
    }

    #[cfg(windows)]
    #[test]
    fn the_pc_shares_with_the_paired_mac_on_the_chosen_side() {
        use sidescreen_core::mouse::MouseRole;
        let (a, _d) = app();
        paired(&a);
        a.set_mac_side(Side::Top).unwrap();
        let MouseRole::Pc(s) = a.mouse_role().unwrap();
        assert_eq!(s.macs.iter().map(|m| m.to_string()).collect::<Vec<_>>(), vec!["192.168.1.50"]);
        assert_eq!(s.token, "t".repeat(64));
        assert_eq!(s.my_id, a.cfg.lock().unwrap().device_id);
        assert_eq!(s.mac_side, Side::Top);
        assert_eq!(s.stream, (2560, 1440));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_mac_shares_with_the_trusted_pc() {
        use sidescreen_core::mouse::MouseRole;
        let (a, _d) = app();
        paired(&a);
        let MouseRole::Mac(s) = a.mouse_role().unwrap();
        assert_eq!((s.pc_id.as_str(), s.token.len()), ("macid", 64));
    }

    #[test]
    fn switching_mouse_and_clipboard_sharing_off_stops_it_and_is_saved() {
        let (a, _d) = app();
        paired(&a);
        a.set_mouse_sharing(false).unwrap();
        a.set_clipboard_sharing(false).unwrap();
        assert!(a.mouse_role().is_none());
        assert!(!Config::load(&a.path).unwrap().mouse_sharing);
        assert!(!a.status().mouse_sharing);
    }

    // 2026-11-18: mouse sharing keeps running while the display is disconnected, and that must
    // be visible, not a surprise.
    #[test]
    fn the_tray_says_when_mouse_sharing_is_on() {
        assert_eq!(tray_text("disconnected", "running"), "Sidescreen: disconnected · mouse sharing on");
        assert_eq!(tray_text("connected", "running"), "Sidescreen: connected · mouse sharing on");
        assert_eq!(tray_text("disconnected", "off"), "Sidescreen: disconnected");
        assert_eq!(tray_text("connected", "failed"), "Sidescreen: connected · mouse sharing not working");
    }

    // 2026-11-19: one quick switch, on either machine, stops all sharing.
    #[test]
    fn pausing_stops_sharing_is_saved_and_shown() {
        let (a, _d) = app();
        paired(&a);
        assert!(a.mouse_role().is_some());
        a.set_sharing_paused(true).unwrap();
        assert!(a.mouse_role().is_none());
        assert!(Config::load(&a.path).unwrap().sharing_paused);
        assert!(a.status().sharing_paused);
        a.set_sharing_paused(false).unwrap();
        assert!(a.mouse_role().is_some());
    }

    #[test]
    fn the_tray_says_when_sharing_is_paused() {
        assert_eq!(tray_text_paused("disconnected"), "Sidescreen: disconnected · sharing paused");
    }

    #[cfg(windows)]
    #[test]
    fn clipboard_and_language_are_on_by_default_and_reach_sharing() {
        use sidescreen_core::mouse::MouseRole;
        let (a, _d) = app();
        paired(&a);
        let s = a.status();
        assert_eq!((s.clipboard_sharing, s.language_follow), (true, true));
        let MouseRole::Pc(p) = a.mouse_role().unwrap();
        assert!(p.mouse && p.clipboard && p.language);
        a.set_clipboard_sharing(false).unwrap();
        a.set_language_follow(false).unwrap();
        let MouseRole::Pc(p) = a.mouse_role().unwrap();
        assert!(!p.clipboard && !p.language);
        assert!(!Config::load(&a.path).unwrap().clipboard_sharing);
    }

    // Clipboard sharing alone still needs the connection to the Mac.
    #[cfg(windows)]
    #[test]
    fn clipboard_alone_keeps_sharing_running() {
        use sidescreen_core::mouse::MouseRole;
        let (a, _d) = app();
        paired(&a);
        a.set_mouse_sharing(false).unwrap();
        let MouseRole::Pc(p) = a.mouse_role().expect("clipboard still shared");
        assert!(!p.mouse && p.clipboard);
        a.set_clipboard_sharing(false).unwrap();
        assert!(a.mouse_role().is_none());
    }

    #[test]
    fn first_run_is_reported_once_and_remembered() {
        let (a, _d) = app();
        assert!(a.take_first_run());
        assert!(!a.take_first_run());
        assert!(Config::load(&a.path).unwrap().first_run_done);
    }

    #[test]
    fn closing_keeps_running_unless_switched_off() {
        let (a, _d) = app();
        assert!(a.status().close_to_tray, "by default closing keeps it in the tray");
        a.set_close_to_tray(false).unwrap();
        assert!(!Config::load(&a.path).unwrap().close_to_tray);
        assert!(!a.status().close_to_tray);
    }

    #[test]
    fn status_reports_mouse_state() {
        let (a, _d) = app();
        assert_eq!(a.status().mouse_state, "off");
        a.set_mouse_status(MouseStatus::Failed("port busy".into()));
        let s = a.status();
        assert_eq!(s.mouse_state, "failed");
        assert_eq!(s.mouse_reason.as_deref(), Some("port busy"));
        assert_eq!(s.mac_side, "right");
    }
}
