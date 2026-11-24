//! The Mac's menu-bar panel: the native helper `sidescreen-panel`, and the JSON lines it and the
//! app exchange over its stdin and stdout.
//!
//! - The app writes one `PanelDto` per line to the helper's stdin, whenever something changed.
//! - The helper writes one `PanelAction` per line to its stdout, when the user does something.
//!
//! The pure part (the state, the actions, the restart rule) is tested on every OS. Nothing here
//! ever logs a line either way: the state carries the pairing code.

use crate::state::{PairingDto, PeerDto, StatusDto, UsesDto};
use crate::system::SystemDto;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

/// One PC in the panel. `state` is "ready", "blocked", "connected" or "connecting".
///
/// "connecting" is never produced yet: the Mac has no per-PC notion of a pending Connect (a
/// Connect only unblocks the PC, which then dials in), so a PC is "ready" until it is connected.
/// The panel accepts the value for when the Mac learns of one.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PanelPc {
    pub key: String,
    pub name: String,
    pub address: String,
    pub state: &'static str,
    /// The PC's own round trip to the Mac; only for the connected PC.
    pub ms: Option<u32>,
    pub uses: Option<UsesDto>,
    /// "2560 × 1440 · 60 fps" while the display is in use and its size is known.
    pub display: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PanelPermissions {
    pub screen: Option<bool>,
    pub accessibility: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PanelDto {
    pub version: String,
    pub connected: Option<PanelPc>,
    pub others: Vec<PanelPc>,
    /// Present only while pairing mode is open.
    pub pairing: Option<PairingDto>,
    pub permissions: PanelPermissions,
    /// The streaming apps this Mac needs but does not have ("Sunshine"); empty when none.
    pub missing: Vec<String>,
    pub stay_awake: bool,
    pub start_at_login: bool,
}

/// "2560 × 1440 · 60 fps".
pub fn display_label(width: u32, height: u32, fps: u32) -> String {
    format!("{width} × {height} · {fps} fps")
}

fn pc(peer: &PeerDto, state: &'static str) -> PanelPc {
    PanelPc {
        key: peer.key.clone(),
        name: peer.name.clone(),
        address: peer.host.clone(),
        state,
        ms: None,
        uses: None,
        display: None,
    }
}

pub fn panel_dto(status: &StatusDto, system: &SystemDto, stay_awake: bool, version: &str) -> PanelDto {
    let mut connected = None;
    let mut others = Vec::new();
    for peer in &status.peers {
        if peer.connected && connected.is_none() {
            let mut c = pc(peer, "connected");
            c.ms = status.latency_ms;
            c.uses = status.uses.clone();
            // The size of the display the Mac is running (the one the PC asked for), while in use.
            if status.uses.as_ref().is_some_and(|u| u.display) {
                c.display = status.display.as_ref().map(|d| display_label(d.width, d.height, d.fps));
            }
            connected = Some(c);
        } else {
            others.push(pc(peer, if peer.allowed { "ready" } else { "blocked" }));
        }
    }
    PanelDto {
        version: version.to_string(),
        connected,
        others,
        pairing: status.pairing.clone(),
        permissions: PanelPermissions { screen: system.screen_recording, accessibility: system.accessibility },
        missing: system.missing.iter().map(|app| app.to_string()).collect(),
        stay_awake,
        start_at_login: system.start_at_login,
    }
}

/// The state as one line for the helper: compact JSON (absent values as `null`) and a newline.
pub fn state_line(dto: &PanelDto) -> String {
    let mut line = serde_json::to_string(dto).expect("the panel state always serializes");
    line.push('\n');
    line
}

/// What the user did in the panel, one per line from the helper, as `{"action":"connect","key":"…"}`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum PanelAction {
    Connect { key: String },
    /// Disconnects the connected PC.
    Disconnect,
    /// Re-runs the connected PC's connection.
    Restart,
    Block { key: String },
    Allow { key: String },
    Forget { key: String },
    /// Opens pairing mode.
    AddPc,
    CancelPairing,
    /// The panel closed: pairing mode, if open, ends with it.
    PanelClosed,
    SetStayAwake { on: bool },
    SetStartAtLogin { on: bool },
    /// Opens the privacy pane for "screen" or "accessibility".
    AllowPermission { kind: String },
    /// Installs what `missing` lists.
    InstallMissing,
    Quit,
}

/// One line from the helper; anything that is not a known action is `None`.
pub fn parse_action(line: &str) -> Option<PanelAction> {
    serde_json::from_str(line.trim()).ok()
}

/// Where an action runs: at once, or in the slow lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lane {
    Now,
    /// Actions that wait on saving the settings or on another program. They run one after
    /// another, in order, so the panel's next actions are never held up behind them.
    Slow,
}

pub fn lane(action: &PanelAction) -> Lane {
    match action {
        PanelAction::Block { .. }
        | PanelAction::Allow { .. }
        | PanelAction::Forget { .. }
        | PanelAction::AllowPermission { .. } => Lane::Slow,
        _ => Lane::Now,
    }
}

/// A helper that exits again this soon after its last exit is not restarted: the tray takes over.
pub const RESTART_WINDOW: Duration = Duration::from_secs(30);

/// Whether a helper that exited at `now` is started again, given when it last exited.
pub fn restart_after_exit(previous_exit: Option<Instant>, now: Instant) -> bool {
    previous_exit.map_or(true, |before| now.saturating_duration_since(before) >= RESTART_WINDOW)
}

/// What follows a helper's exit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AfterExit {
    /// The app is ending (quitting, or asked to by the panel or the system): nothing.
    Stop,
    Restart,
    /// It exited twice too soon: the tray takes over.
    Tray,
}

/// `ending` when the app is closing or a quit was asked for: a helper that exits then (say, at
/// log out, before the app has finished) is not taken for a crash.
pub fn after_exit(ending: bool, previous_exit: Option<Instant>, now: Instant) -> AfterExit {
    if ending {
        AfterExit::Stop
    } else if restart_after_exit(previous_exit, now) {
        AfterExit::Restart
    } else {
        AfterExit::Tray
    }
}

#[cfg(target_os = "macos")]
pub use helper::{push, running, start, stop};

/// Running the helper: started at launch, fed the state, its actions carried out, restarted
/// once if it exits, and given up for the tray if it exits again within `RESTART_WINDOW`.
#[cfg(target_os = "macos")]
mod helper {
    use super::*;
    use crate::Shared;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;
    use tauri::{AppHandle, Manager};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    use tokio::process::{Child, ChildStdin, Command};
    use tokio::sync::watch;

    /// The latest state line, while the panel is in use. Each helper's writer follows it, so a
    /// restarted helper gets the current state at once. `None`: no panel (the tray is up).
    static LINE: Mutex<Option<watch::Sender<String>>> = Mutex::new(None);
    /// The app is quitting: a helper that exits now is neither restarted nor replaced.
    static CLOSING: AtomicBool = AtomicBool::new(false);
    /// The panel asked to quit (its Quit, or a quit from the system): from then on its exit is
    /// expected, even before the app has wound down.
    static QUIT_ASKED: AtomicBool = AtomicBool::new(false);

    fn ending() -> bool {
        CLOSING.load(Ordering::SeqCst) || QUIT_ASKED.load(Ordering::SeqCst)
    }
    /// The running helper's process id, until it has exited and been reaped.
    static PID: Mutex<Option<u32>> = Mutex::new(None);
    /// How long a helper asked to end gets before it is killed.
    const END_GRACE: Duration = Duration::from_secs(1);

    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    const SIGTERM: i32 = 15;
    const SIGKILL: i32 = 9;

    /// Asks the process to end (SIGTERM) and kills it if it is still `alive` after `grace`.
    fn terminate(pid: u32, grace: Duration, alive: impl Fn() -> bool) {
        // 0 or a negative id would signal a whole process group.
        let Ok(pid) = i32::try_from(pid) else { return };
        if pid <= 0 {
            return;
        }
        unsafe { kill(pid, SIGTERM) };
        let until = Instant::now() + grace;
        while Instant::now() < until {
            if !alive() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        if alive() {
            unsafe { kill(pid, SIGKILL) };
        }
    }

    /// The helper ships inside the app, next to its executable.
    fn helper_path() -> Option<std::path::PathBuf> {
        let path = std::env::current_exe().ok()?.parent()?.join("sidescreen-panel");
        path.exists().then_some(path)
    }

    fn launch(exe: &std::path::Path) -> std::io::Result<Child> {
        Command::new(exe)
            // Its lifeline: the helper quits when its stdin closes, so it never outlives the app.
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            // Nothing it says goes to a log.
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
    }

    /// Starts the panel. `false` when there is no helper: the caller puts up the tray. `on_gone`
    /// is called (from a background task) if the panel is given up later, including when the
    /// helper does not start at all.
    pub fn start(app: &AppHandle, on_gone: fn(&AppHandle)) -> bool {
        let Some(exe) = helper_path() else { return false };
        *LINE.lock().unwrap() = Some(watch::channel(String::new()).0);
        // Launched inside the runtime: tokio's process pipes need its reactor.
        tauri::async_runtime::spawn(supervise(app.clone(), exe, on_gone));
        true
    }

    /// Whether the panel is in use (rather than the tray).
    pub fn running() -> bool {
        LINE.lock().unwrap().is_some()
    }

    /// Sends the state to the panel. Unless `always`, only when it differs from the last sent.
    pub fn push(dto: &PanelDto, always: bool) {
        let line = state_line(dto);
        let guard = LINE.lock().unwrap();
        let Some(tx) = guard.as_ref() else { return };
        if always {
            tx.send_replace(line);
        } else {
            tx.send_if_modified(|last| {
                let changed = *last != line;
                if changed {
                    *last = line;
                }
                changed
            });
        }
    }

    /// The app is quitting: the helper's stdin closes and it is asked to end (then killed after
    /// `END_GRACE`), so no second menu-bar icon stays behind; it is not restarted. Blocks for at
    /// most `END_GRACE`, usually a few milliseconds.
    pub fn stop() {
        if CLOSING.swap(true, Ordering::SeqCst) {
            return;
        }
        LINE.lock().unwrap().take();
        let pid = *PID.lock().unwrap();
        if let Some(pid) = pid {
            terminate(pid, END_GRACE, || *PID.lock().unwrap() == Some(pid));
        }
    }

    async fn supervise(app: AppHandle, exe: std::path::PathBuf, on_gone: fn(&AppHandle)) {
        let mut child = match launch(&exe) {
            Ok(child) => child,
            Err(e) => {
                eprintln!("sidescreen: the panel did not start ({e}); using the menu instead");
                LINE.lock().unwrap().take();
                if !ending() {
                    on_gone(&app);
                }
                return;
            }
        };
        let mut last_exit = None;
        loop {
            serve(&app, &mut child).await;
            let now = Instant::now();
            match after_exit(ending(), last_exit, now) {
                AfterExit::Stop => return,
                AfterExit::Restart => {}
                AfterExit::Tray => {
                    eprintln!("sidescreen: the panel exited twice within {}s; using the menu instead", RESTART_WINDOW.as_secs());
                    break;
                }
            }
            last_exit = Some(now);
            eprintln!("sidescreen: the panel exited; starting it again");
            match launch(&exe) {
                Ok(next) => child = next,
                Err(e) => {
                    eprintln!("sidescreen: the panel did not start again ({e}); using the menu instead");
                    break;
                }
            }
        }
        LINE.lock().unwrap().take();
        if !ending() {
            on_gone(&app);
        }
    }

    /// Runs one helper until it exits: the state goes in, its actions are carried out.
    async fn serve(app: &AppHandle, child: &mut Child) {
        // Both are taken out before `wait`, which would otherwise close stdin itself.
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        *PID.lock().unwrap() = child.id();
        let follow = LINE.lock().unwrap().as_ref().map(|tx| tx.subscribe());
        let writer = match (stdin, follow) {
            (Some(stdin), Some(follow)) => Some(tauri::async_runtime::spawn(write_lines(stdin, follow))),
            _ => None,
        };
        // The slow lane: its actions in order, apart from the rest.
        let (slow, mut queue) = tokio::sync::mpsc::unbounded_channel::<PanelAction>();
        let lane_app = app.clone();
        tauri::async_runtime::spawn(async move {
            while let Some(action) = queue.recv().await {
                dispatch(&lane_app, action).await;
            }
        });
        if let Some(stdout) = stdout {
            let mut reader = tokio::io::BufReader::new(stdout);
            let mut line = Vec::new();
            // The lines themselves are never logged, and junk (not even text) is dropped
            // without a word.
            loop {
                line.clear();
                match reader.read_until(b'\n', &mut line).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        if let Some(action) = std::str::from_utf8(&line).ok().and_then(parse_action) {
                            match lane(&action) {
                                Lane::Slow => drop(slow.send(action)),
                                Lane::Now => dispatch(app, action).await,
                            }
                        }
                    }
                }
            }
        }
        // The slow lane finishes what it was given, then ends.
        drop(slow);
        // Dropping the writer closes stdin, the lifeline, in case the helper is still running.
        if let Some(writer) = writer {
            writer.abort();
        }
        let _ = child.wait().await;
        *PID.lock().unwrap() = None;
    }

    /// Writes the current state, then every new one, until the helper or the panel is gone.
    async fn write_lines(mut stdin: ChildStdin, mut follow: watch::Receiver<String>) {
        loop {
            let line = follow.borrow_and_update().clone();
            if !line.is_empty() && (stdin.write_all(line.as_bytes()).await.is_err() || stdin.flush().await.is_err()) {
                return;
            }
            if follow.changed().await.is_err() {
                return;
            }
        }
    }

    /// Carries out one action with the same functions the window's commands use, then sends the
    /// state again even if unchanged, so a toggle the panel flipped is put right when it failed.
    async fn dispatch(app: &AppHandle, action: PanelAction) {
        let shared: Shared = app.state::<Shared>().inner().clone();
        match action {
            PanelAction::Connect { key } => {
                // Reaching the PC can take a while; other actions are not held up meanwhile.
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = crate::connect_pc(app.state::<Shared>(), key).await {
                        eprintln!("sidescreen: connecting from the panel: {e}");
                    }
                    let shared = app.state::<Shared>().inner().clone();
                    crate::push_panel(&app, &shared, true);
                });
                return;
            }
            PanelAction::Disconnect => crate::cut_pc(&shared, None),
            PanelAction::Restart => {
                let _ = crate::restart_pc(app.state::<Shared>());
            }
            PanelAction::Block { key } => {
                let _ = crate::set_pc_allowed(app.clone(), app.state::<Shared>(), key, false).await;
            }
            PanelAction::Allow { key } => {
                let _ = crate::set_pc_allowed(app.clone(), app.state::<Shared>(), key, true).await;
            }
            PanelAction::Forget { key } => {
                let _ = crate::forget_pc(app.clone(), app.state::<Shared>(), key).await;
            }
            // The result holds the code: dropped unread.
            PanelAction::AddPc => drop(crate::start_pairing(app.clone(), app.state::<Shared>())),
            PanelAction::CancelPairing => crate::cancel_pairing(app.clone(), app.state::<Shared>()),
            PanelAction::PanelClosed => {
                if shared.pairing_open().is_some() {
                    crate::cancel_pairing(app.clone(), app.state::<Shared>());
                }
            }
            PanelAction::SetStayAwake { on } => {
                let _ = crate::set_stay_awake(app.clone(), app.state::<Shared>(), on);
            }
            PanelAction::SetStartAtLogin { on } => {
                let _ = crate::system::set_start_at_login(app.clone(), on);
            }
            PanelAction::AllowPermission { kind } => {
                let _ = crate::open_privacy_settings(kind);
            }
            PanelAction::InstallMissing => {
                if let Err(e) = crate::system::install_missing() {
                    eprintln!("sidescreen: installing from the panel: {e}");
                }
            }
            PanelAction::Quit => {
                // Marked first: should the helper end before the app (a quit from the system
                // gives it 5 s), that is not a crash to restart.
                QUIT_ASKED.store(true, Ordering::SeqCst);
                // The exit's wind-down stops the helper (`stop`), off this task.
                app.exit(0);
                return;
            }
        }
        crate::push_panel(app, &shared, true);
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use tokio::io::AsyncReadExt;

        fn exits_within(child: &mut std::process::Child, limit: Duration) -> bool {
            let until = Instant::now() + limit;
            while Instant::now() < until {
                if child.try_wait().unwrap().is_some() {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            false
        }

        #[test]
        fn a_helper_is_asked_to_end_and_ends() {
            let child = std::process::Command::new("/bin/sleep").arg("30").spawn().unwrap();
            let pid = child.id();
            let child = std::sync::Mutex::new(child);
            terminate(pid, Duration::from_millis(500), || child.lock().unwrap().try_wait().unwrap().is_none());
            assert!(exits_within(&mut child.lock().unwrap(), Duration::from_secs(2)));
        }

        #[test]
        fn a_helper_that_ignores_the_request_is_killed_after_the_grace() {
            // Ignores SIGTERM (the disposition survives the exec).
            let mut child = std::process::Command::new("/bin/sh")
                .args(["-c", "trap '' TERM; exec /bin/sleep 30"])
                .spawn()
                .unwrap();
            std::thread::sleep(Duration::from_millis(200));
            let pid = child.id();
            let child_lock = std::sync::Mutex::new(child);
            terminate(pid, Duration::from_millis(300), || child_lock.lock().unwrap().try_wait().unwrap().is_none());
            child = child_lock.into_inner().unwrap();
            assert!(exits_within(&mut child, Duration::from_secs(2)));
        }

        /// `cat` stands in for the helper: it echoes what it is sent and exits when its stdin
        /// closes, as the helper does.
        #[tokio::test]
        async fn the_writer_sends_the_latest_state_and_closing_it_ends_the_helper() {
            let mut child = launch(std::path::Path::new("/bin/cat")).expect("cat starts");
            let (stdin, mut stdout) = (child.stdin.take().unwrap(), child.stdout.take().unwrap());
            let (tx, rx) = watch::channel(String::new());
            tx.send_replace("{\"one\":1}\n".into());
            let writer = tokio::spawn(write_lines(stdin, rx));
            let mut got = vec![0u8; 10];
            tokio::time::timeout(Duration::from_secs(5), stdout.read_exact(&mut got)).await.unwrap().unwrap();
            assert_eq!(got, b"{\"one\":1}\n");
            tx.send_replace("{\"two\":2}\n".into());
            tokio::time::timeout(Duration::from_secs(5), stdout.read_exact(&mut got)).await.unwrap().unwrap();
            assert_eq!(got, b"{\"two\":2}\n");
            // The panel is gone: the writer ends, stdin closes, and the helper exits.
            drop(tx);
            tokio::time::timeout(Duration::from_secs(5), writer).await.unwrap().unwrap();
            let status = tokio::time::timeout(Duration::from_secs(5), child.wait()).await.unwrap().unwrap();
            assert!(status.success());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{DisplayDto, PlacementDto, SettingsDto};

    fn peer(key: &str, host: &str, allowed: bool, connected: bool) -> PeerDto {
        PeerDto {
            key: key.into(),
            name: format!("PC {key}"),
            host: host.into(),
            addresses: Vec::new(),
            current: false,
            allowed,
            paired: true,
            connected,
        }
    }

    fn status(peers: Vec<PeerDto>) -> StatusDto {
        StatusDto {
            role: "host",
            state: "disconnected",
            part: None,
            reason: None,
            peer_name: None,
            peer_host: None,
            paired: false,
            settings: SettingsDto { width: 2560, height: 1440, fps: 60, bitrate_mbps: 30, window_mode: "windowed".into(), monitor: None },
            config_error: None,
            mouse_sharing: true,
            mac_side: "right",
            mouse_state: "running",
            mouse_reason: None,
            connection: "off",
            busy_by: None,
            peers,
            auto_connect: true,
            placement: PlacementDto { side: "right", monitor: None },
            uses: None,
            pairing: None,
            close_to_tray: true,
            clipboard_sharing: true,
            language_follow: true,
            start_display_on_connect: false,
            ctrl_as_cmd: true,
            peer_addresses: Vec::new(),
            latency_ms: None,
            viewer_monitor: None,
            display: None,
        }
    }

    fn system() -> SystemDto {
        SystemDto { missing: None, start_at_login: true, screen_recording: None, accessibility: None }
    }

    #[test]
    fn a_connected_pc_leads_with_its_uses_and_ms_and_the_others_follow() {
        let mut s = status(vec![peer("a", "192.168.1.2", true, false), peer("b", "192.168.1.3", true, true)]);
        s.uses = Some(UsesDto { mouse: true, clipboard: false, display: false });
        s.latency_ms = Some(7);
        let p = panel_dto(&s, &system(), true, "1.2.3");
        let c = p.connected.expect("connected");
        assert_eq!((c.key.as_str(), c.state, c.ms, c.address.as_str()), ("b", "connected", Some(7), "192.168.1.3"));
        assert_eq!(c.uses.map(|u| (u.mouse, u.clipboard, u.display)), Some((true, false, false)));
        assert_eq!(c.display, None);
        assert_eq!(p.others.len(), 1);
        assert_eq!((p.others[0].key.as_str(), p.others[0].state, p.others[0].ms), ("a", "ready", None));
        assert_eq!(p.version, "1.2.3");
        assert!(p.stay_awake && p.start_at_login);
    }

    #[test]
    fn a_blocked_pc_is_blocked() {
        let p = panel_dto(&status(vec![peer("a", "192.168.1.2", false, false)]), &system(), true, "1");
        assert_eq!(p.others[0].state, "blocked");
        assert!(p.connected.is_none());
    }

    #[test]
    fn no_pcs_gives_empty_lists() {
        let p = panel_dto(&status(Vec::new()), &system(), false, "1");
        assert!(p.connected.is_none() && p.others.is_empty());
        assert!(!p.stay_awake);
    }

    #[test]
    fn the_pairing_code_passes_through_only_while_open() {
        let mut s = status(Vec::new());
        assert!(panel_dto(&s, &system(), true, "1").pairing.is_none());
        s.pairing = Some(PairingDto { code: "123456".into(), seconds_left: 112 });
        let p = panel_dto(&s, &system(), true, "1").pairing.expect("open");
        assert_eq!((p.code.as_str(), p.seconds_left), ("123456", 112));
    }

    #[test]
    fn permissions_pass_through_and_stay_unknown_when_unknown() {
        let p = panel_dto(&status(Vec::new()), &system(), true, "1");
        assert_eq!(p.permissions, PanelPermissions { screen: None, accessibility: None });
        let mut sys = system();
        sys.screen_recording = Some(true);
        sys.accessibility = Some(false);
        let p = panel_dto(&status(Vec::new()), &sys, true, "1");
        assert_eq!(p.permissions, PanelPermissions { screen: Some(true), accessibility: Some(false) });
    }

    #[test]
    fn the_display_label_uses_the_middle_dot_and_the_times_sign() {
        assert_eq!(display_label(2560, 1440, 60), "2560 × 1440 · 60 fps");
    }

    #[test]
    fn every_action_parses_from_its_line() {
        let cases = [
            (r#"{"action":"connect","key":"abc"}"#, PanelAction::Connect { key: "abc".into() }),
            (r#"{"action":"disconnect"}"#, PanelAction::Disconnect),
            (r#"{"action":"restart"}"#, PanelAction::Restart),
            (r#"{"action":"block","key":"abc"}"#, PanelAction::Block { key: "abc".into() }),
            (r#"{"action":"allow","key":"abc"}"#, PanelAction::Allow { key: "abc".into() }),
            (r#"{"action":"forget","key":"abc"}"#, PanelAction::Forget { key: "abc".into() }),
            (r#"{"action":"add_pc"}"#, PanelAction::AddPc),
            (r#"{"action":"cancel_pairing"}"#, PanelAction::CancelPairing),
            (r#"{"action":"panel_closed"}"#, PanelAction::PanelClosed),
            (r#"{"action":"set_stay_awake","on":true}"#, PanelAction::SetStayAwake { on: true }),
            (r#"{"action":"set_start_at_login","on":false}"#, PanelAction::SetStartAtLogin { on: false }),
            (r#"{"action":"allow_permission","kind":"screen"}"#, PanelAction::AllowPermission { kind: "screen".into() }),
            (r#"{"action":"install_missing"}"#, PanelAction::InstallMissing),
            (r#"{"action":"quit"}"#, PanelAction::Quit),
        ];
        for (line, want) in cases {
            assert_eq!(parse_action(line), Some(want), "{line}");
        }
        // Surrounding whitespace and the line's own newline are fine; extra fields are ignored.
        assert_eq!(parse_action("  {\"action\":\"quit\"}\r\n"), Some(PanelAction::Quit));
        assert_eq!(parse_action(r#"{"action":"disconnect","extra":1}"#), Some(PanelAction::Disconnect));
    }

    #[test]
    fn unknown_actions_and_junk_give_nothing() {
        for line in [
            "",
            "   ",
            "hello",
            "{",
            "null",
            "[]",
            r#"{"key":"abc"}"#,
            r#"{"action":"format_disk"}"#,
            r#"{"action":"Connect","key":"abc"}"#,
            r#"{"action":"connect"}"#,
            r#"{"action":"connect","key":7}"#,
            r#"{"action":"set_stay_awake","on":"yes"}"#,
            r#"{"action":"quit"} {"action":"quit"}"#,
        ] {
            assert_eq!(parse_action(line), None, "{line}");
        }
    }

    #[test]
    fn the_panel_state_line_has_exactly_the_fields_the_panel_decodes() {
        let mut s = status(vec![peer("pc-b", "192.168.1.3", true, true), peer("pc-a", "192.168.1.2", false, false)]);
        s.uses = Some(UsesDto { mouse: true, clipboard: true, display: true });
        s.latency_ms = Some(4);
        s.display = Some(DisplayDto { width: 2560, height: 1440, fps: 60 });
        s.pairing = Some(PairingDto { code: "123456".into(), seconds_left: 112 });
        let mut sys = system();
        sys.screen_recording = Some(true);
        sys.accessibility = Some(false);
        sys.missing = Some("Sunshine");
        let line = state_line(&panel_dto(&s, &sys, true, "1.1.0"));
        assert_eq!(
            line,
            concat!(
                r#"{"version":"1.1.0","#,
                r#""connected":{"key":"pc-b","name":"PC pc-b","address":"192.168.1.3","state":"connected","ms":4,"#,
                r#""uses":{"mouse":true,"clipboard":true,"display":true},"display":"2560 × 1440 · 60 fps"},"#,
                r#""others":[{"key":"pc-a","name":"PC pc-a","address":"192.168.1.2","state":"blocked","ms":null,"uses":null,"display":null}],"#,
                r#""pairing":{"code":"123456","seconds_left":112},"#,
                r#""permissions":{"screen":true,"accessibility":false},"#,
                r#""missing":["Sunshine"],"#,
                r#""stay_awake":true,"start_at_login":true}"#,
                "\n"
            )
        );
        // Nothing connected and pairing closed: those are null.
        let line = state_line(&panel_dto(&status(Vec::new()), &system(), false, "1.1.0"));
        assert!(line.contains(r#""connected":null,"others":[],"pairing":null"#), "{line}");
        assert!(line.contains(r#""missing":[]"#), "{line}");
        assert_eq!(line.matches('\n').count(), 1);
    }

    #[test]
    fn slow_actions_go_to_their_own_lane_and_the_rest_run_at_once() {
        let key = || "pc-a".to_string();
        for slow in [
            PanelAction::Block { key: key() },
            PanelAction::Allow { key: key() },
            PanelAction::Forget { key: key() },
            PanelAction::AllowPermission { kind: "screen".into() },
        ] {
            assert_eq!(lane(&slow), Lane::Slow, "{slow:?}");
        }
        for now in [
            PanelAction::Connect { key: key() },
            PanelAction::Disconnect,
            PanelAction::Restart,
            PanelAction::AddPc,
            PanelAction::CancelPairing,
            PanelAction::PanelClosed,
            PanelAction::SetStayAwake { on: true },
            PanelAction::SetStartAtLogin { on: false },
            PanelAction::InstallMissing,
            PanelAction::Quit,
        ] {
            assert_eq!(lane(&now), Lane::Now, "{now:?}");
        }
    }

    #[test]
    fn a_helper_that_exits_twice_within_30_seconds_is_not_restarted() {
        let start = Instant::now();
        // The first exit is restarted.
        assert!(restart_after_exit(None, start));
        // A second one soon after is not: the tray takes over.
        assert!(!restart_after_exit(Some(start), start + Duration::from_secs(5)));
        assert!(!restart_after_exit(Some(start), start + Duration::from_millis(29_999)));
        // Exits spaced out are restarted each time.
        assert!(restart_after_exit(Some(start), start + Duration::from_secs(30)));
        assert!(restart_after_exit(Some(start), start + Duration::from_secs(600)));
    }

    #[test]
    fn a_helper_that_exits_after_a_quit_was_asked_is_neither_restarted_nor_replaced() {
        let start = Instant::now();
        assert_eq!(after_exit(true, None, start), AfterExit::Stop);
        assert_eq!(after_exit(true, Some(start), start + Duration::from_secs(1)), AfterExit::Stop);
        assert_eq!(after_exit(false, None, start), AfterExit::Restart);
        assert_eq!(after_exit(false, Some(start), start + Duration::from_secs(1)), AfterExit::Tray);
    }

    #[test]
    fn the_display_shows_when_in_use_and_a_size_is_running() {
        let mut s = status(vec![peer("a", "192.168.1.2", true, true)]);
        s.uses = Some(UsesDto { mouse: true, clipboard: true, display: true });
        // In use, but no running size known.
        assert_eq!(panel_dto(&s, &system(), true, "1").connected.unwrap().display, None);
        s.display = Some(DisplayDto { width: 2560, height: 1440, fps: 60 });
        assert_eq!(panel_dto(&s, &system(), true, "1").connected.unwrap().display.as_deref(), Some("2560 × 1440 · 60 fps"));
        // A size known but the display not in use.
        s.uses = Some(UsesDto { mouse: true, clipboard: true, display: false });
        assert_eq!(panel_dto(&s, &system(), true, "1").connected.unwrap().display, None);
    }
}
