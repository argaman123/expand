//! The Mac side lives in the menu bar: this decides what its menu says. Pure, so it is tested
//! on any machine; `lib.rs` turns it into a real menu.

use crate::state::StatusDto;
use crate::system::SystemDto;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    /// Information, not clickable.
    Text(String),
    /// A command. `enabled: false` shows it greyed out.
    Action { id: &'static str, label: String, enabled: bool },
    /// An on/off setting.
    Check { id: &'static str, label: String, checked: bool },
    Separator,
}

pub fn host_menu(status: &StatusDto, system: &SystemDto) -> Vec<Line> {
    let now = match status.state {
        "connected" => "Showing the display".to_string(),
        "connecting" => "Starting…".to_string(),
        "failed" => short(&format!(
            "{}: {}",
            status.part.as_deref().unwrap_or("Problem"),
            status.reason.as_deref().unwrap_or("unknown")
        )),
        _ => "Ready".to_string(),
    };
    let mut lines = vec![Line::Text(now)];
    // The Mac has no "current" PC: it names the connected one, or the only one.
    let paired: Vec<_> = status.peers.iter().filter(|p| p.paired).collect();
    let named = paired.iter().find(|p| p.connected).or(if paired.len() == 1 { paired.first() } else { None });
    match (named, paired.len()) {
        (_, 0) => lines.push(Line::Text("No PC paired yet".into())),
        (Some(pc), _) => lines.push(Line::Text(short(&format!("PC: {}", pc.name)))),
        (None, n) => lines.push(Line::Text(format!("{n} PCs paired"))),
    }
    // Interim (the menu is replaced in plan 2): only the one PC there is can be forgotten here.
    if paired.len() == 1 {
        let idle = matches!(status.state, "disconnected" | "failed");
        lines.push(Line::Action { id: "forget", label: "Forget this PC".into(), enabled: idle });
    }

    lines.push(Line::Separator);
    let sharing = if status.mouse_sharing && status.mouse_state == "failed" {
        "Sharing isn't working"
    } else {
        "Sharing mouse, keyboard and clipboard"
    };
    lines.push(Line::Text(sharing.into()));
    lines.push(Line::Check { id: "login", label: "Start at login".into(), checked: system.start_at_login });

    // Granted permissions need no attention: only a missing one shows as something to do.
    let missing_permission = [
        ("perm-screen", "Screen Recording", system.screen_recording),
        ("perm-access", "Accessibility", system.accessibility),
    ];
    if missing_permission.iter().any(|(_, _, granted)| *granted == Some(false)) || system.missing.is_some() {
        lines.push(Line::Separator);
    }
    for (id, name, granted) in missing_permission {
        match granted {
            Some(false) => lines.push(Line::Action { id, label: format!("Allow {name}…"), enabled: true }),
            Some(true) if name == "Screen Recording" && system.accessibility == Some(false) => {
                lines.push(Line::Text(format!("{name} ✓")))
            }
            _ => {}
        }
    }
    if let Some(app) = system.missing {
        lines.push(Line::Action { id: "install", label: format!("Install {app}…"), enabled: true });
    }

    lines.push(Line::Separator);
    lines.push(Line::Action { id: "quit", label: "Quit Expand".into(), enabled: true });
    lines
}

/// Menu lines stay short; anything longer is cut with an ellipsis.
fn short(text: &str) -> String {
    const MAX: usize = 40;
    if text.chars().count() <= MAX {
        text.to_string()
    } else {
        format!("{}…", text.chars().take(MAX - 1).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{PeerDto, SettingsDto};

    fn pc(key: &str, name: &str, connected: bool) -> PeerDto {
        PeerDto {
            key: key.into(),
            name: name.into(),
            host: "192.168.1.10".into(),
            addresses: Vec::new(),
            current: false,
            allowed: true,
            paired: true,
            connected,
        }
    }

    fn status() -> StatusDto {
        StatusDto {
            role: "host",
            state: "disconnected",
            part: None,
            reason: None,
            peer_name: Some("DESKTOP-PC".into()),
            peer_host: Some("192.168.1.10".into()),
            paired: true,
            settings: SettingsDto { width: 2560, height: 1440, fps: 60, bitrate_mbps: 30, window_mode: "windowed".into(), monitor: None },
            config_error: None,
            mouse_sharing: true,
            mac_side: "right",
            mouse_state: "running",
            mouse_reason: None,
            connection: "off",
            busy_by: None,
            peers: vec![pc("pc-1", "DESKTOP-PC", false)],
            auto_connect: true,
            placement: crate::state::PlacementDto { side: "right", monitor: None },
            uses: None,
            pairing: None,
            close_to_tray: true,
            clipboard_sharing: true,
            language_follow: true,
            start_display_on_connect: false,
            ctrl_as_cmd: true,
            reverse_scroll: true,
            mac_sound: true,
            peer_addresses: Vec::new(),
            latency_ms: None,
            viewer_monitor: None,
            display: None,
        }
    }

    fn system() -> SystemDto {
        SystemDto { missing: None, start_at_login: true, screen_recording: Some(true), accessibility: Some(false) }
    }

    fn texts(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .filter_map(|l| match l {
                Line::Text(t) => Some(t.clone()),
                Line::Action { label, .. } | Line::Check { label, .. } => Some(label.clone()),
                Line::Separator => None,
            })
            .collect()
    }

    #[test]
    fn the_menu_starts_with_what_is_happening_and_with_whom() {
        let lines = host_menu(&status(), &system());
        assert_eq!(lines[0], Line::Text("Ready".into()));
        assert_eq!(lines[1], Line::Text("PC: DESKTOP-PC".into()));
    }

    // The Mac no longer has a current PC (final review I6): the menu names the connected one.
    #[test]
    fn with_several_pcs_the_menu_names_the_connected_one() {
        let mut two = status();
        two.peers.push(pc("pc-2", "LAPTOP", false));
        let lines = host_menu(&two, &system());
        assert_eq!(lines[1], Line::Text("2 PCs paired".into()));
        assert!(!texts(&lines).contains(&"Forget this PC".to_string()));
        two.peers[1].connected = true;
        assert_eq!(host_menu(&two, &system())[1], Line::Text("PC: LAPTOP".into()));
    }

    // 2026-11-19: "way too long"; one short line says what is shared.
    #[test]
    fn sharing_has_one_short_line() {
        let lines = host_menu(&status(), &system());
        assert!(lines.contains(&Line::Text("Sharing mouse, keyboard and clipboard".into())));
        assert!(!lines.iter().any(|l| matches!(l, Line::Check { id: "pause", .. })));
    }

    #[test]
    fn every_label_is_short() {
        let mut failed = status();
        failed.state = "failed";
        failed.part = Some("Sunshine".into());
        failed.reason = Some("already running".into());
        for line in texts(&host_menu(&status(), &system())).into_iter().chain(texts(&host_menu(&failed, &system()))) {
            assert!(line.chars().count() <= 40, "too long for a menu: {line:?}");
        }
    }

    #[test]
    fn a_missing_permission_is_a_link_to_grant_it_and_a_granted_one_is_ticked() {
        let lines = texts(&host_menu(&status(), &system()));
        assert!(lines.contains(&"Screen Recording ✓".to_string()), "{lines:?}");
        assert!(!lines.iter().any(|l| l.contains("Accessibility ✓")), "{lines:?}");
        assert!(lines.contains(&"Allow Accessibility…".to_string()), "{lines:?}");
    }

    #[test]
    fn start_at_login_forget_and_quit_are_there() {
        let lines = host_menu(&status(), &system());
        assert!(lines.contains(&Line::Check { id: "login", label: "Start at login".into(), checked: true }));
        assert!(lines.contains(&Line::Action { id: "forget", label: "Forget this PC".into(), enabled: true }));
        assert_eq!(lines.last(), Some(&Line::Action { id: "quit", label: "Quit Expand".into(), enabled: true }));
    }

    #[test]
    fn forgetting_is_not_offered_mid_session_nor_without_a_pc() {
        let mut connected = status();
        connected.state = "connected";
        let lines = host_menu(&connected, &system());
        assert!(lines.contains(&Line::Action { id: "forget", label: "Forget this PC".into(), enabled: false }));
        assert_eq!(lines[0], Line::Text("Showing the display".into()));

        let mut alone = status();
        alone.paired = false;
        alone.peer_name = None;
        alone.peer_host = None;
        alone.peers.clear();
        let lines = host_menu(&alone, &system());
        assert_eq!(lines[1], Line::Text("No PC paired yet".into()));
        assert!(!texts(&lines).contains(&"Forget this PC".to_string()));
    }

    #[test]
    fn problems_are_spelled_out() {
        let mut failed = status();
        failed.state = "failed";
        failed.part = Some("Sunshine".into());
        failed.reason = Some("already running".into());
        failed.mouse_state = "failed";
        failed.mouse_reason = Some("Accessibility permission is missing".into());
        let lines = texts(&host_menu(&failed, &SystemDto { missing: Some("Sunshine"), ..system() }));
        assert!(lines.contains(&"Sunshine: already running".to_string()), "{lines:?}");
        assert!(lines.contains(&"Sharing isn't working".to_string()), "{lines:?}");
        assert!(lines.contains(&"Install Sunshine…".to_string()), "{lines:?}");
    }
}
