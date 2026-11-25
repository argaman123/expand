//! What the app remembers while running, kept free of window code so it can be tested.

use serde::Serialize;
use expand_core::config::{Config, Peer, Placement, StreamSettings, WindowMode};
use expand_core::side::Side;
use expand_core::mouse::{EndReason, MouseRole, MouseStatus, Uses};
use expand_core::share::admit::KnownPc;
use expand_core::session::State;
use std::path::PathBuf;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Serialize)]
pub struct SettingsDto {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_mbps: u32,
    pub window_mode: String,
    /// The monitor number the viewer opens on; `None`: toward the Mac.
    pub monitor: Option<u32>,
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
    /// "off", "connecting", "connected", "lost", "ended_by_mac", "busy", "blocked", "unknown" or
    /// "version" (the other computer must be updated):
    /// the connection to the other machine, apart from the display (`state`).
    pub connection: &'static str,
    /// Who the Mac is in use by, when `connection` is "busy".
    pub busy_by: Option<String>,
    pub peers: Vec<PeerDto>,
    pub auto_connect: bool,
    pub placement: PlacementDto,
    /// What the connected PC uses the Mac for (Mac side).
    pub uses: Option<UsesDto>,
    /// The pairing code the Mac is showing (Mac side).
    pub pairing: Option<PairingDto>,
    /// Closing the window keeps the app running in the background.
    pub close_to_tray: bool,
    pub clipboard_sharing: bool,
    pub language_follow: bool,
    /// PC: the display starts when the connection to the Mac comes up.
    pub start_display_on_connect: bool,
    /// PC: Ctrl reaches the Mac as Command (and Windows as Control) while it has the keyboard.
    pub ctrl_as_cmd: bool,
    /// PC: the wheel scrolls the Mac the other way round (sharing and the display).
    pub reverse_scroll: bool,
    /// PC: the Mac's sound plays on the PC while the display runs.
    pub mac_sound: bool,
    /// Other addresses the Mac reported (Tailscale, a cable link, another network).
    pub peer_addresses: Vec<String>,
    /// PC side: the smoothed round trip to the connected Mac, in ms.
    pub latency_ms: Option<u32>,
    /// PC side: the 1-based number of the monitor the Mac display opens on (Windows only).
    pub viewer_monitor: Option<u32>,
    /// Mac side: the size of the display the Mac is running for a PC, while it runs.
    pub display: Option<DisplayDto>,
}

/// Whether the display starts as the connection to the Mac is `now` (PC, with the option
/// `enabled`): when it is connected, the user `asked` to connect (Connect, a connect at launch, or
/// the Mac asking), and no display is `running` yet. Never inferred from the connection's own
/// comings and goings: it restarts by itself (a new address list), and comes back after a loss,
/// and neither reopens a display the user closed.
pub fn starts_display_on_connect(asked: bool, now: &MouseStatus, enabled: bool, running: bool) -> bool {
    asked && matches!(now, MouseStatus::Connected { .. }) && enabled && !running
}

/// What happens to a display that ended ([`AppState::display_ended`]): closed by the user it stays
/// closed. Ended by itself, it comes back with the connection to the Mac (the Mac slept or left
/// the network), at once when the connection is back already. On a connection that never
/// dropped it shows why it ended, with Retry, instead of starting over and over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Comeback {
    No,
    Now,
    WhenConnected,
}

/// Admits a PC only while pairing mode is open and it proves it has the code.
pub struct PairingGate(pub Arc<AppState>);

impl expand_core::link::PairGate for PairingGate {
    fn code(&self) -> Option<String> {
        self.0.pairing_code_at(Instant::now())
    }

    fn conclude(&self, code: &str, proved: bool) -> bool {
        self.0.conclude_pairing_at(code, proved, Instant::now())
    }
}

/// The pairing in progress, or `None` once it has run out (and then it is cleared, so an expired
/// pairing mode is closed everywhere).
fn live_pairing(pairing: &mut Option<(String, Instant)>, now: Instant) -> Option<&(String, Instant)> {
    if pairing.as_ref().is_some_and(|(_, until)| now >= *until) {
        *pairing = None;
    }
    pairing.as_ref()
}

/// Pairing exchanges that may end without the code before pairing mode closes: each one lets
/// the PC (or whoever is on the network) test one code, and the code has only a million values.
pub const PAIRING_ATTEMPTS: u32 = 5;

#[derive(Debug, Clone, Serialize)]
pub struct PeerDto {
    pub key: String,
    pub name: String,
    pub host: String,
    pub addresses: Vec<String>,
    /// The Mac the PC is working with (PC side); the Mac has no current PC.
    pub current: bool,
    pub allowed: bool,
    pub paired: bool,
    /// Connected on the sharing channel right now: on the Mac, the PC holding it; on the PC, the
    /// Mac it is connected to.
    pub connected: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DisplayDto {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

/// Wraps the Mac's engines to note the size of the display while it runs: set when a start
/// succeeds, cleared when the engines stop or one dies.
pub struct SizeTracking<'a, E> {
    pub inner: E,
    pub state: &'a AppState,
}

#[async_trait::async_trait]
impl<E: expand_core::session::HostEngines> expand_core::session::HostEngines for SizeTracking<'_, E> {
    async fn start(&mut self, width: u32, height: u32, fps: u32) -> Result<(), expand_core::sunshine::EngineError> {
        let result = self.inner.start(width, height, fps).await;
        self.state.set_display_size(result.is_ok().then_some(DisplayDto { width, height, fps }));
        result
    }
    async fn submit_pin(&mut self, pin: &str, client_name: &str) -> Result<bool, expand_core::sunshine::EngineError> {
        self.inner.submit_pin(pin, client_name).await
    }
    async fn stop(&mut self) {
        self.state.set_display_size(None);
        self.inner.stop().await;
    }
    async fn exited(&mut self) -> &'static str {
        let part = self.inner.exited().await;
        self.state.set_display_size(None);
        part
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PlacementDto {
    pub side: &'static str,
    pub monitor: Option<u32>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct UsesDto {
    pub mouse: bool,
    pub clipboard: bool,
    pub display: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PairingDto {
    pub code: String,
    pub seconds_left: u32,
}

/// The tray's tooltip: the display's state, and mouse sharing whenever it is on.
pub fn tray_text(state: &str, mouse_state: &str) -> String {
    match mouse_state {
        "running" => format!("Expand: {state} · mouse sharing on"),
        "failed" => format!("Expand: {state} · mouse sharing not working"),
        _ => format!("Expand: {state}"),
    }
}

/// One command in the PC tray; `checked` is `Some` for the two toggles.
#[derive(Debug, Clone, PartialEq)]
pub struct TrayAction {
    pub id: &'static str,
    pub label: &'static str,
    pub enabled: bool,
    pub checked: Option<bool>,
}

/// The PC tray's commands between "Open" and "Quit": the Mouse and keyboard toggle, the Extended
/// display toggle (checked while the display is starting or up, and then always usable to stop
/// it, whatever the link does; otherwise usable once connected), and Connect when not connected
/// or Disconnect when connected.
pub fn client_tray_actions(status: &StatusDto) -> Vec<TrayAction> {
    let connected = status.connection == "connected";
    let display_on = matches!(status.state, "connecting" | "connected");
    let toggle = |id, label, enabled, on| TrayAction { id, label, enabled, checked: Some(on) };
    vec![
        toggle("mouse", "Mouse and keyboard", true, status.mouse_sharing),
        toggle("display", "Extended display", connected || display_on, display_on),
        if connected {
            TrayAction { id: "disconnect", label: "Disconnect", enabled: true, checked: None }
        } else {
            TrayAction { id: "connect", label: "Connect", enabled: status.paired || status.peer_host.is_some(), checked: None }
        },
    ]
}

/// How long the Mac's pairing mode stays open.
pub const PAIRING_WINDOW: Duration = Duration::from_secs(120);

pub const ROLE: &str = if cfg!(target_os = "macos") { "host" } else { "client" };

pub struct AppState {
    pub cfg: Mutex<Config>,
    pub path: PathBuf,
    pub state: watch::Sender<State>,
    active: Mutex<Option<CancellationToken>>,
    config_error: Mutex<Option<String>>,
    /// The mouse-sharing engine's last reported status.
    mouse_status: Mutex<MouseStatus>,
    /// Whether the user wants the connection to the Mac up. Starts from the auto-connect
    /// setting; Disconnect and every end for good turn it off until Connect.
    want_connection: AtomicBool,
    /// The user asked to connect, and the connection has not come up for it yet (for "start the
    /// display when connected").
    connect_asked: AtomicBool,
    /// The connection to the Mac dropped while the running display was up.
    dropped_under_display: AtomicBool,
    /// A display that ended by itself waits to come back with the connection.
    comeback: AtomicBool,
    /// The pairing code the Mac is showing and when it stops working (Mac side).
    pairing: Mutex<Option<(String, Instant)>>,
    pairing_signal: watch::Sender<u64>,
    /// Wrong proofs seen in this pairing mode.
    pairing_misses: AtomicU32,
    /// The size of the display the Mac is running (Mac side).
    display_size: Mutex<Option<DisplayDto>>,
    /// The PC holding the display (Mac side).
    display_holder: Mutex<Option<String>>,
    /// Each PC's open display connection, to cut it (Mac side).
    controls: Mutex<HashMap<String, (u64, CancellationToken)>>,
    control_seq: AtomicU64,
    /// Orders for the Mac's sharing server (Task 8 sends `Disconnect` through the sender).
    #[cfg(target_os = "macos")]
    pub mac_commands: MacCommands,
    /// The PCs the Mac's sharing server lets in, updated with every saved change so a running
    /// server follows it without restarting.
    #[cfg(target_os = "macos")]
    mac_pcs: watch::Sender<Vec<KnownPc>>,
    /// A Restart under way (Mac side): which PC, since when, and whether a Disconnect came
    /// meanwhile and must be carried out when the PC is back.
    restart: Mutex<Option<Restart>>,
}

/// How long after a Restart the PC counts as restarting: a Disconnect in that time cuts it when
/// it comes back.
pub const RESTART_CUT_WINDOW: Duration = Duration::from_secs(15);

struct Restart {
    pc: String,
    at: Instant,
    cut: bool,
}

/// Puts `new` in place of the entry `old_key`, keeping what the user set there (where it sits,
/// how it streams, whether it is allowed, Moonlight's pairing) and whatever `new` leaves empty.
/// With no such entry, `new` is added only when `insert`. `current` follows a renamed entry and
/// never moves otherwise. Returns the entry's key, or `None` when nothing was stored.
fn merge_peer(cfg: &mut Config, old_key: &str, mut new: Peer, insert: bool) -> Option<String> {
    let Some(i) = cfg.peers.iter().position(|p| p.key() == old_key) else {
        return insert.then(|| cfg.upsert_peer(new));
    };
    let old = cfg.peers[i].clone();
    new.placement = old.placement;
    new.stream = old.stream;
    new.allowed = old.allowed;
    new.sunshine_paired = old.sunshine_paired;
    if new.token.is_empty() {
        new.token = old.token;
    }
    if new.host.is_empty() {
        new.host = old.host;
    }
    if new.addresses.is_empty() {
        new.addresses = old.addresses;
    }
    let key = new.key();
    let was_current = cfg.current.as_deref() == Some(old_key);
    match cfg.peers.iter().position(|p| p.key() == key) {
        // Renamed onto another entry that already has this key: that one is replaced.
        Some(j) if j != i => {
            cfg.peers[j] = new;
            cfg.peers.remove(i);
        }
        _ => cfg.peers[i] = new,
    }
    if was_current {
        cfg.current = Some(key.clone());
    }
    Some(key)
}

/// How many other addresses are kept for a peer.
const MAX_ADDRESSES: usize = 8;

/// The peer was reached at `ip`: that is its address now; the old one goes first among the
/// others (de-duplicated, at most [`MAX_ADDRESSES`]).
fn moved_to(peer: &mut Peer, ip: &str) {
    if peer.host == ip {
        return;
    }
    let old = std::mem::replace(&mut peer.host, ip.to_string());
    let mut addresses = vec![old];
    addresses.append(&mut peer.addresses);
    addresses.retain(|a| !a.is_empty() && a != ip);
    let mut seen = std::collections::HashSet::new();
    addresses.retain(|a| seen.insert(a.clone()));
    addresses.truncate(MAX_ADDRESSES);
    peer.addresses = addresses;
}

/// The PCs the Mac's sharing server knows: every paired one (a PC without a token never paired).
pub fn known_pcs(cfg: &Config) -> Vec<KnownPc> {
    cfg.peers
        .iter()
        .filter(|p| !p.token.is_empty())
        .map(|p| (p.id.clone(), p.token.clone(), p.allowed, p.name.clone()))
        .collect()
}

/// The PC calling as `id`, by the rule the sharing server uses
/// ([`expand_core::share::admit::entry`]): the entry with that id, or else the single paired
/// entry without one (a PC migrated from 1.0), given the caller's id. It must still prove itself
/// with that entry's token.
pub fn known_pc(cfg: &Config, id: &str) -> Option<Peer> {
    let pcs = known_pcs(cfg);
    let (found, token, ..) = expand_core::share::admit::entry(&pcs, id)?;
    // Only paired entries are in `pcs`: one without a token proves nothing, by id or not.
    let pc = cfg.peers.iter().find(|p| &p.id == found && &p.token == token)?;
    Some(Peer { id: id.to_string(), ..pc.clone() })
}

/// The sender the app uses and the receiver the Mac's server takes when it starts.
#[cfg(target_os = "macos")]
pub struct MacCommands(
    pub std::sync::Arc<Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<expand_core::share::mac::MacCommand>>>>,
    pub tokio::sync::mpsc::UnboundedSender<expand_core::share::mac::MacCommand>,
);

#[cfg(target_os = "macos")]
impl Default for MacCommands {
    fn default() -> Self {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        MacCommands(std::sync::Arc::new(Mutex::new(Some(rx))), tx)
    }
}

/// The number of the monitor the Mac display opens on, as the viewer window picks it (Windows only).
#[cfg(windows)]
fn viewer_monitor_number(peer: Option<&Peer>) -> Option<u32> {
    use expand_core::{layout, winwindow};
    let peer = peer?;
    let monitors = winwindow::monitors();
    let area = layout::viewer_monitor(&monitors, peer.stream.monitor, &peer.placement)?;
    layout::monitor_number(&monitors, &area)
}

#[cfg(not(windows))]
fn viewer_monitor_number(_peer: Option<&Peer>) -> Option<u32> {
    None
}

fn prefs_of(cfg: &Config) -> expand_core::moonlight::Prefs {
    expand_core::moonlight::Prefs { reverse_scroll: cfg.reverse_scroll, sound_here: cfg.mac_sound }
}

impl AppState {
    pub fn new(cfg: Config, path: PathBuf, config_error: Option<String>) -> AppState {
        expand_core::moonlight::set_prefs(prefs_of(&cfg));
        AppState {
            want_connection: AtomicBool::new(cfg.auto_connect),
            connect_asked: AtomicBool::new(false),
            dropped_under_display: AtomicBool::new(false),
            comeback: AtomicBool::new(false),
            path,
            state: watch::channel(State::Disconnected).0,
            active: Mutex::new(None),
            config_error: Mutex::new(config_error),
            mouse_status: Mutex::new(MouseStatus::Off),
            pairing: Mutex::new(None),
            pairing_signal: watch::channel(0).0,
            pairing_misses: AtomicU32::new(0),
            display_size: Mutex::new(None),
            display_holder: Mutex::new(None),
            controls: Mutex::new(HashMap::new()),
            control_seq: AtomicU64::new(1),
            #[cfg(target_os = "macos")]
            mac_pcs: watch::channel(known_pcs(&cfg)).0,
            #[cfg(target_os = "macos")]
            mac_commands: MacCommands::default(),
            restart: Mutex::new(None),
            cfg: Mutex::new(cfg),
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
        self.dropped_under_display.store(false, Ordering::SeqCst);
        self.comeback.store(false, Ordering::SeqCst);
        Some(token)
    }

    /// The display ended, `stopped` by the user (or a restart) or by itself, with the connection
    /// to the Mac `now`: whether it comes back, and when ([`Comeback`]).
    pub fn display_ended(&self, stopped: bool, now: &MouseStatus) -> Comeback {
        let dropped = self.dropped_under_display.swap(false, Ordering::SeqCst);
        let up = matches!(now, MouseStatus::Connected { .. });
        let comeback = match (stopped, up, dropped) {
            (true, _, _) => Comeback::No,
            (false, true, true) => Comeback::Now,
            (false, true, false) => Comeback::No,
            (false, false, _) => Comeback::WhenConnected,
        };
        self.comeback.store(comeback == Comeback::WhenConnected, Ordering::SeqCst);
        comeback
    }

    /// Whether a display waiting to come back starts now: the connection has just come back
    /// (`was_connected` says it was not up before `now`), and no display runs. Once.
    pub fn resumes_display_now(&self, was_connected: bool, now: &MouseStatus) -> bool {
        if was_connected || !matches!(now, MouseStatus::Connected { .. }) || self.has_session() {
            return false;
        }
        self.comeback.swap(false, Ordering::SeqCst)
    }

    /// The user closed the display (or disconnected): nothing brings it back.
    pub fn call_off_comeback(&self) {
        self.comeback.store(false, Ordering::SeqCst);
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
        // The running sharing server follows the list at once (before anything else can ask).
        #[cfg(target_os = "macos")]
        self.mac_pcs.send_if_modified(|pcs| {
            let now = known_pcs(&cfg);
            let changed = *pcs != now;
            *pcs = now;
            changed
        });
        cfg.save(&self.path).map_err(|e| e.to_string())
    }

    pub fn save_settings(&self, s: StreamSettings) -> Result<(), String> {
        let ok = (640..=7680).contains(&s.width)
            && (360..=4320).contains(&s.height)
            && (10..=240).contains(&s.fps)
            && (1_000..=150_000).contains(&s.bitrate_kbps);
        if !ok {
            return Err("those settings are out of range".to_string());
        }
        self.update_current(|peer| peer.stream = s)
    }

    /// Saves how the display window shows (full screen or a window) and, when given, the monitor
    /// it fills. Only that: the stream itself is untouched, so a running display goes on.
    pub fn save_display_mode(&self, mode: WindowMode, monitor: Option<u32>) -> Result<(), String> {
        self.update_current(|peer| {
            peer.stream.window_mode = mode;
            if monitor.is_some() {
                peer.stream.monitor = monitor;
            }
        })
    }

    /// Points the app at a Mac by address. It is not trusted until the first connect pairs.
    pub fn set_peer_host(&self, host: &str) -> Result<(), String> {
        let host = host.trim();
        if host.parse::<std::net::IpAddr>().is_err() {
            return Err("enter the Mac's IP address, for example 192.168.1.50".to_string());
        }
        self.update(|cfg| {
            // Interim: the app still works with one Mac, so this replaces the current one and
            // keeps where it sits and how it streams.
            let (placement, stream) = cfg
                .current_peer()
                .map(|p| (p.placement, p.stream.clone()))
                .unwrap_or_default();
            if let Some(old) = cfg.current.clone() {
                cfg.remove_peer(&old);
            }
            let key = cfg.upsert_peer(Peer {
                name: host.to_string(),
                id: String::new(),
                host: host.to_string(),
                token: String::new(),
                sunshine_paired: false,
                addresses: Vec::new(),
                placement,
                stream,
                allowed: true,
            });
            cfg.current = Some(key);
        })
    }

    pub fn set_peer(&self, peer: Peer) -> Result<(), String> {
        self.update(|cfg| {
            let key = cfg.upsert_peer(peer);
            cfg.current = Some(key);
        })
    }

    /// Changes the current peer; there is nothing to change before a Mac is chosen.
    fn update_current(&self, change: impl FnOnce(&mut Peer)) -> Result<(), String> {
        if self.cfg.lock().unwrap().current_peer().is_none() {
            return Err("choose a Mac first".to_string());
        }
        self.update(|cfg| {
            if let Some(peer) = cfg.current_peer_mut() {
                change(peer);
            }
        })
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

    pub fn set_ctrl_as_cmd(&self, enabled: bool) -> Result<(), String> {
        self.update(|cfg| cfg.ctrl_as_cmd = enabled)
    }

    /// The next display started follows too ([`stream_prefs`](Self::stream_prefs)).
    pub fn set_reverse_scroll(&self, enabled: bool) -> Result<(), String> {
        self.update(|cfg| cfg.reverse_scroll = enabled)?;
        expand_core::moonlight::set_prefs(self.stream_prefs());
        Ok(())
    }

    /// From the next display started ([`stream_prefs`](Self::stream_prefs)).
    pub fn set_mac_sound(&self, enabled: bool) -> Result<(), String> {
        self.update(|cfg| cfg.mac_sound = enabled)?;
        expand_core::moonlight::set_prefs(self.stream_prefs());
        Ok(())
    }

    /// What the next display starts with, from the settings.
    pub fn stream_prefs(&self) -> expand_core::moonlight::Prefs {
        prefs_of(&self.cfg.lock().unwrap())
    }

    pub fn set_start_display_on_connect(&self, enabled: bool) -> Result<(), String> {
        self.update(|cfg| cfg.start_display_on_connect = enabled)
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

    pub fn want_connection(&self) -> bool {
        self.want_connection.load(Ordering::SeqCst)
    }

    pub fn set_want_connection(&self, want: bool) {
        self.want_connection.store(want, Ordering::SeqCst);
    }

    /// The user asked to connect (Connect, a connect at launch, the Mac asking): the next time
    /// the connection is up, the display may start ([`starts_display_now`](Self::starts_display_now)).
    pub fn ask_connect(&self, asked: bool) {
        self.connect_asked.store(asked, Ordering::SeqCst);
    }

    /// At launch: the connect auto-connect makes is the user's ask, when a connection will run
    /// at all (a paired Mac, the connection wanted).
    pub fn ask_connect_at_launch(&self) {
        self.ask_connect(self.mouse_role().is_some());
    }

    /// What the connection should run as ([`mouse_role`](Self::mouse_role)). When it should be
    /// down (disconnected, no paired Mac), an ask to connect is dropped: it can't come up for it,
    /// and a later link-up must not open the display for it.
    pub fn connection_role(&self) -> Option<MouseRole> {
        let role = self.mouse_role();
        if role.is_none() {
            self.ask_connect(false);
        }
        role
    }

    /// Whether the display starts now that the connection is `now` ([`starts_display_on_connect`]).
    /// A connection up uses the ask (once); one ended for good drops it; a loss or a restart on
    /// the way keeps it.
    pub fn starts_display_now(&self, now: &MouseStatus) -> bool {
        match now {
            MouseStatus::Connected { .. } => {
                let asked = self.connect_asked.swap(false, Ordering::SeqCst);
                let enabled = self.cfg.lock().unwrap().start_display_on_connect;
                starts_display_on_connect(asked, now, enabled, self.has_session())
            }
            MouseStatus::Ended(_) => {
                self.connect_asked.store(false, Ordering::SeqCst);
                false
            }
            _ => false,
        }
    }

    /// The mouse-sharing engine's last reported status.
    pub fn mouse_status(&self) -> MouseStatus {
        self.mouse_status.lock().unwrap().clone()
    }

    pub fn set_auto_connect(&self, enabled: bool) -> Result<(), String> {
        self.update(|cfg| cfg.auto_connect = enabled)
    }

    /// Makes a saved Mac the current one. Switching to another Mac ends the display of the old
    /// one. True when it switched.
    pub fn make_current(&self, key: &str) -> Result<bool, String> {
        let switching = {
            let cfg = self.cfg.lock().unwrap();
            if cfg.peer(key).is_none() {
                return Err("that Mac is not in the list".to_string());
            }
            cfg.current.as_deref() != Some(key)
        };
        if switching {
            self.cancel_session();
            self.update(|cfg| cfg.current = Some(key.to_string()))?;
        }
        Ok(switching)
    }

    /// Pairs with the Mac `peer` points at, using its code, over the control link alone (at
    /// `port`): nothing is stored and no display starts. Pairs even when a token is saved: a
    /// typed code is how a stale or rejected token is replaced. Returns the Mac as it answered:
    /// its id and name, the new token and the address that answered.
    pub async fn pair_with_code(&self, peer: &Peer, code: &str, port: u16) -> Result<Peer, String> {
        let addrs: Vec<std::net::SocketAddr> =
            peer.candidates().into_iter().map(|ip| std::net::SocketAddr::new(ip, port)).collect();
        if addrs.is_empty() {
            return Err("the saved Mac address is not valid".to_string());
        }
        let (name, id) = {
            let cfg = self.cfg.lock().unwrap();
            (cfg.device_name.clone(), cfg.device_id.clone())
        };
        let (mut conn, new_peer, _) =
            expand_core::link::client::connect(&addrs, &name, &id, None, Some(code)).await.map_err(|e| e.to_string())?;
        // Nothing more is wanted from this connection.
        let _ = conn.send(expand_core::protocol::Message::Stop).await;
        new_peer.ok_or_else(|| "the Mac did not pair".to_string())
    }

    /// Brings the connection to a Mac up: the chosen one (`key`) or the current one, pairing
    /// with `code` first when one is typed (or required: there is no token yet). Nothing is
    /// switched until that has worked, so a wrong code leaves the working connection alone; a
    /// switch to another Mac ends the old one's display. The caller then syncs sharing.
    pub async fn connect_to(&self, key: Option<&str>, code: Option<&str>, port: u16) -> Result<(), String> {
        let peer = {
            let cfg = self.cfg.lock().unwrap();
            match key {
                Some(key) => cfg.peer(key).cloned().ok_or("that Mac is not in the list")?,
                None => cfg.current_peer().cloned().ok_or("choose a Mac first")?,
            }
        };
        self.connect_peer(peer, code, port).await
    }

    /// Adds a Mac by address: it is saved only once it has paired with `code`, so a wrong code or
    /// a mistyped address leaves the list as it was. A Mac already saved at that address is
    /// paired again instead, as [`AppState::connect_to`] does.
    pub async fn add_mac_by_address(&self, host: &str, code: &str, port: u16) -> Result<(), String> {
        let host = host.trim();
        if host.parse::<std::net::IpAddr>().is_err() {
            return Err("enter the Mac's IP address, for example 192.168.1.50".to_string());
        }
        let known = self.cfg.lock().unwrap().peers.iter().find(|p| p.host == host).map(|p| p.key());
        if let Some(key) = known {
            return self.connect_to(Some(&key), Some(code), port).await;
        }
        let unsaved = Peer {
            name: host.to_string(),
            id: String::new(),
            host: host.to_string(),
            token: String::new(),
            sunshine_paired: false,
            addresses: Vec::new(),
            placement: Default::default(),
            stream: Default::default(),
            allowed: true,
        };
        self.connect_peer(unsaved, Some(code), port).await
    }

    /// [`AppState::connect_to`] for `peer`, which need not be saved yet: it is stored only once
    /// pairing has worked.
    async fn connect_peer(&self, peer: Peer, code: Option<&str>, port: u16) -> Result<(), String> {
        let code = code.map(str::trim).filter(|c| !c.is_empty());
        let key = match code {
            Some(code) => {
                let new = self.pair_with_code(&peer, code, port).await?;
                // Pairing may rename the entry (by address → by the Mac's id). An unsaved Mac
                // that turns out to be saved under its id already is that entry.
                let old_key = {
                    let cfg = self.cfg.lock().unwrap();
                    match cfg.peer(&peer.key()) {
                        Some(_) => peer.key(),
                        None => cfg
                            .peers
                            .iter()
                            .find(|p| !new.id.is_empty() && p.id == new.id)
                            .map(|p| p.key())
                            .unwrap_or_else(|| peer.key()),
                    }
                };
                self.replace_peer(&old_key, new)?
            }
            None if peer.token.is_empty() => return Err("Enter the Mac's code".to_string()),
            None => peer.key(),
        };
        self.make_current(&key)?;
        self.set_want_connection(true);
        Ok(())
    }

    /// Gives a saved Mac a new address, keeping its pairing (token, id, settings): the old one
    /// is kept among its other addresses. Returns whether it is the current Mac, or `None` when the
    /// address is the one it already has (nothing is saved).
    pub fn set_mac_address(&self, key: &str, host: &str) -> Result<Option<bool>, String> {
        let host = host.trim();
        if self.check_mac_address(key, host)?.is_none() {
            return Ok(None);
        }
        let mut was_current = false;
        self.update(|cfg| {
            let Some(i) = cfg.peers.iter().position(|p| p.key() == key) else { return };
            was_current = cfg.current.as_deref() == Some(key);
            moved_to(&mut cfg.peers[i], host);
            // A Mac not yet known by its id is keyed by its address.
            if was_current {
                cfg.current = Some(cfg.peers[i].key());
            }
        })?;
        Ok(Some(was_current))
    }

    /// Whether a saved Mac may take `host` as its address, changing nothing: `Ok(None)` when it
    /// is the address it has, else its key once moved there (a Mac known by address alone is
    /// keyed by it). Refused: not an IP address, no such Mac, or another Mac's address.
    pub fn check_mac_address(&self, key: &str, host: &str) -> Result<Option<String>, String> {
        let host = host.trim();
        if host.parse::<std::net::IpAddr>().is_err() {
            return Err("enter the Mac's IP address, for example 192.168.1.50".to_string());
        }
        let cfg = self.cfg.lock().unwrap();
        let peer = cfg.peer(key).ok_or("that Mac is not in the list")?;
        if peer.host == host {
            return Ok(None);
        }
        // An id-less Mac is keyed by its address, so the new key must not meet another's.
        let new_key = if peer.id.is_empty() { format!("host:{host}") } else { peer.key() };
        if cfg.peers.iter().any(|p| p.key() != key && (p.host == host || p.key() == new_key)) {
            return Err("That address belongs to another Mac".to_string());
        }
        Ok(Some(new_key))
    }

    /// Gives a saved Mac a new address, as [`AppState::set_mac_address`], checked before anything
    /// is stopped: a refused address leaves a running display alone. The current Mac's display is
    /// stopped first (and waited for, up to 4 s) so it neither keeps the old address nor loses
    /// what it learnt when it writes back. Returns whether it was the current Mac (`None`: the
    /// address it had, nothing changed) and its key now, to connect to.
    pub async fn change_mac_address(&self, key: &str, host: &str) -> Result<(Option<bool>, String), String> {
        let Some(new_key) = self.check_mac_address(key, host)? else {
            return Ok((None, key.to_string()));
        };
        let is_current = self.cfg.lock().unwrap().current.as_deref() == Some(key);
        if is_current && self.cancel_session() {
            let mut waited = 0;
            while self.has_session() && waited < 4000 {
                tokio::time::sleep(Duration::from_millis(50)).await;
                waited += 50;
            }
        }
        let was_current = self.set_mac_address(key, host)?;
        Ok((was_current, new_key))
    }

    /// Saves a Mac by address (or finds the one already saved there) and returns its key.
    pub fn add_peer_by_host(&self, host: &str) -> Result<String, String> {
        let host = host.trim();
        if host.parse::<std::net::IpAddr>().is_err() {
            return Err("enter the Mac's IP address, for example 192.168.1.50".to_string());
        }
        if let Some(known) = self.cfg.lock().unwrap().peers.iter().find(|p| p.host == host) {
            return Ok(known.key());
        }
        let mut key = String::new();
        self.update(|cfg| {
            key = cfg.upsert_peer(Peer {
                name: host.to_string(),
                id: String::new(),
                host: host.to_string(),
                token: String::new(),
                sunshine_paired: false,
                addresses: Vec::new(),
                placement: Default::default(),
                stream: Default::default(),
                allowed: true,
            });
        })?;
        Ok(key)
    }

    /// Stores a peer that arrived from the other machine (pairing, or the Mac's real id),
    /// keeping what the user set on the entry it replaces: where it sits, how it streams,
    /// whether it is allowed. Added when there is no such entry. Which Mac is current does not
    /// change (a renamed current entry stays current). Returns its key.
    pub fn replace_peer(&self, old_key: &str, new: Peer) -> Result<String, String> {
        let mut key = String::new();
        self.update(|cfg| key = merge_peer(cfg, old_key, new, true).expect("added when missing"))?;
        Ok(key)
    }

    /// What a display session learnt about the Mac (Moonlight's pairing, the addresses it
    /// reported), written only onto an entry that is still there: a Mac forgotten meanwhile stays
    /// forgotten, and which Mac is current, its token and the user's settings are left alone.
    pub fn write_back_session(&self, old_key: &str, after: &Peer) {
        let _ = self.update(|cfg| {
            let at = cfg.peers.iter().position(|p| p.key() == old_key).or_else(|| {
                // Learnt its id meanwhile (by address → by id): the same Mac.
                old_key.strip_prefix("host:").and_then(|host| cfg.peers.iter().position(|p| p.host == host))
            });
            if let Some(i) = at {
                cfg.peers[i].sunshine_paired = after.sunshine_paired;
                cfg.peers[i].addresses = after.addresses.clone();
            }
        });
    }

    /// A pairing that happened inside a display session: stored like [`AppState::replace_peer`],
    /// but only onto an entry that is still there.
    pub fn write_back_pairing(&self, old_key: &str, new: Peer) {
        let _ = self.update(|cfg| {
            merge_peer(cfg, old_key, new, false);
        });
    }

    /// The PC learnt who the Mac really is: the entry saved by address gets its id and name.
    pub fn learn_mac(&self, mac_id: &str, mac_name: &str) {
        if mac_id.is_empty() {
            return;
        }
        let Some(old) = self.cfg.lock().unwrap().current_peer().cloned() else { return };
        // Only an entry saved by address learns the id; a known id is never overwritten. The
        // same Mac under a new name (it could not read its computer name before) is renamed.
        if !old.id.is_empty() {
            if old.id == mac_id && !mac_name.is_empty() && old.name != mac_name {
                let _ = self.update(|cfg| {
                    if let Some(p) = cfg.peers.iter_mut().find(|p| p.id == mac_id) {
                        p.name = mac_name.to_string();
                    }
                });
            }
            return;
        }
        let known = Peer { id: mac_id.to_string(), name: mac_name.to_string(), ..old.clone() };
        let _ = self.replace_peer(&old.key(), known);
    }

    /// A PC proved itself as the entry without an id (migrated from 1.0): that entry is it, and
    /// is known by its id from now on. `current` follows it.
    pub fn learn_pc(&self, pc_id: &str) {
        if let Some(seen) = self.pc_to_learn(pc_id) {
            self.adopt_pc(pc_id, &seen);
        }
    }

    /// The id-less entry the PC calling as `pc_id` resolves to, when it still has to be learnt.
    fn pc_to_learn(&self, pc_id: &str) -> Option<Peer> {
        let cfg = self.cfg.lock().unwrap();
        if cfg.peers.iter().any(|p| p.id == pc_id) || known_pc(&cfg, pc_id).is_none() {
            return None;
        }
        cfg.peers.iter().find(|p| p.id.is_empty() && !p.token.is_empty()).cloned()
    }

    /// Writes the learnt id onto `seen`, the entry [`AppState::pc_to_learn`] found, checked again
    /// on the live settings: only while it is still there, still without an id and with the same
    /// token, and no entry has that id yet. Nothing is ever added, so a PC forgotten meanwhile
    /// stays forgotten, and a second learning of the same PC changes nothing.
    fn adopt_pc(&self, pc_id: &str, seen: &Peer) {
        let _ = self.update(|cfg| {
            if pc_id.is_empty() || cfg.peers.iter().any(|p| p.id == pc_id) {
                return;
            }
            let key = seen.key();
            let Some(pc) = cfg.peers.iter_mut().find(|p| p.id.is_empty() && p.key() == key && p.token == seen.token) else {
                return;
            };
            pc.id = pc_id.to_string();
            if cfg.current.as_deref() == Some(key.as_str()) {
                cfg.current = Some(pc_id.to_string());
            }
        });
    }

    pub fn forget_mac(&self, key: &str) -> Result<(), String> {
        if self.cfg.lock().unwrap().peer(key).is_none() {
            return Err("that Mac is not in the list".to_string());
        }
        self.update(|cfg| {
            cfg.remove_peer(key);
        })
    }

    pub fn set_placement(&self, placement: Placement) -> Result<(), String> {
        self.update_current(|peer| peer.placement = placement)
    }

    pub fn set_mouse_status(&self, status: MouseStatus) {
        if matches!(status, MouseStatus::Lost(_) | MouseStatus::Connecting) && self.has_session() {
            self.dropped_under_display.store(true, Ordering::SeqCst);
        }
        match &status {
            // Ended for good: nothing may start it again until the user connects.
            MouseStatus::Ended(_) => self.set_want_connection(false),
            // The PC just learnt the Mac's identity in the handshake.
            MouseStatus::Connected { mac_id, mac_name, .. } if ROLE == "client" => self.learn_mac(mac_id, mac_name),
            // The Mac learns the id of a PC migrated from 1.0, then where the PC is now (its
            // address may have changed since pairing).
            MouseStatus::Connected { mac_id, from, .. } if ROLE == "host" => {
                self.learn_pc(mac_id);
                if let Some(ip) = from {
                    self.saw_pc_at(mac_id, *ip);
                }
            }
            _ => {}
        }
        *self.mouse_status.lock().unwrap() = status;
    }

    /// What the connection to the Mac should run as, or `None` when it should be down: the user
    /// disconnected, or there is no paired Mac. Mouse and clipboard are settings of the running
    /// connection, never a reason to drop it.
    pub fn mouse_role(&self) -> Option<MouseRole> {
        let cfg = self.cfg.lock().unwrap();
        // The Mac serves every paired PC, whichever one the app is looking at.
        #[cfg(target_os = "macos")]
        {
            if !known_pcs(&cfg).iter().any(|p| p.2) {
                return None;
            }
            return Some(MouseRole::Mac(expand_core::share::mac::MacSettings {
                me: expand_core::share::channel::MacIdentity { id: cfg.device_id.clone(), name: cfg.device_name.clone() },
                pcs: self.mac_pcs.subscribe(),
                commands: self.mac_commands.0.clone(),
            }));
        }
        #[cfg(windows)]
        {
            if !self.want_connection() {
                return None;
            }
            let peer = cfg.current_peer().filter(|p| !p.token.is_empty())?;
            return Some(MouseRole::Pc(expand_core::share::pc::PcSettings {
                macs: Some(peer.candidates()).filter(|c| !c.is_empty())?,
                my_id: cfg.device_id.clone(),
                token: peer.token.clone(),
                placement: peer.placement.clone(),
                stream: (peer.stream.width, peer.stream.height),
                // Mouse off stops everything on the connection, which itself stays up.
                toggles: expand_core::mouse::Toggles {
                    mouse: cfg.mouse_sharing,
                    clipboard: cfg.mouse_sharing && cfg.clipboard_sharing,
                    language: cfg.mouse_sharing && cfg.language_follow,
                    display: self.has_session(),
                    ctrl_as_cmd: cfg.ctrl_as_cmd,
                    reverse_scroll: cfg.reverse_scroll,
                },
            }));
        }
        #[allow(unreachable_code)]
        None
    }

    // ---- the Mac: pairing mode, several PCs ----

    /// Opens pairing mode with a fresh code. Only a PC that proves it has the code can pair, and
    /// only until [`PAIRING_WINDOW`] passes or one PC has.
    pub fn start_pairing(&self) -> String {
        self.start_pairing_at(Instant::now())
    }

    pub fn start_pairing_at(&self, now: Instant) -> String {
        let code = expand_core::auth::new_pair_code();
        {
            // The count starts again with the new mode, under the lock the misses are counted
            // under, so an exchange ending meanwhile counts against one mode or the other.
            let mut pairing = self.pairing.lock().unwrap();
            *pairing = Some((code.clone(), now + PAIRING_WINDOW));
            self.pairing_misses.store(0, Ordering::SeqCst);
        }
        self.pairing_signal.send_modify(|n| *n += 1);
        code
    }

    pub fn cancel_pairing(&self) {
        if self.pairing.lock().unwrap().take().is_some() {
            self.pairing_signal.send_modify(|n| *n += 1);
        }
    }

    /// The code and the whole seconds left (rounded up), while pairing mode is open.
    pub fn pairing_open(&self) -> Option<(String, u32)> {
        self.pairing_open_at(Instant::now())
    }

    pub fn pairing_open_at(&self, now: Instant) -> Option<(String, u32)> {
        let mut pairing = self.pairing.lock().unwrap();
        let (code, until) = live_pairing(&mut pairing, now)?;
        Some((code.clone(), (*until - now).as_millis().div_ceil(1000) as u32))
    }

    /// When pairing mode is open, how long until it ends by itself; `None` when it is closed.
    pub fn pairing_ends_in(&self) -> Option<Duration> {
        self.pairing_ends_in_at(Instant::now())
    }

    pub fn pairing_ends_in_at(&self, now: Instant) -> Option<Duration> {
        let mut pairing = self.pairing.lock().unwrap();
        live_pairing(&mut pairing, now).map(|(_, until)| *until - now)
    }

    /// The code a PC pairs with, while pairing mode is open.
    pub fn pairing_code_at(&self, now: Instant) -> Option<String> {
        self.pairing_open_at(now).map(|(code, _)| code)
    }

    /// Ends one pairing exchange that ran with `code`; `proved` when the PC proved it has it.
    /// Only the pairing mode still open with that code admits (an exchange can outlast the
    /// mode it started in), and it closes when one PC has paired or [`PAIRING_ATTEMPTS`]
    /// exchanges have ended without the code. True when the PC may be paired.
    pub fn conclude_pairing_at(&self, code: &str, proved: bool, now: Instant) -> bool {
        let mut pairing = self.pairing.lock().unwrap();
        // Checked and counted under one lock, so exchanges ending together cannot slip past
        // the count or past a mode that has just closed.
        if live_pairing(&mut pairing, now).is_none_or(|(open, _)| open != code) {
            return false;
        }
        let close = proved || self.pairing_misses.fetch_add(1, Ordering::SeqCst) + 1 >= PAIRING_ATTEMPTS;
        if close {
            *pairing = None;
            drop(pairing);
            self.pairing_signal.send_modify(|n| *n += 1);
        }
        proved
    }

    /// Changes whenever pairing mode opens or closes by an act (not by running out).
    pub fn pairing_changes(&self) -> watch::Receiver<u64> {
        self.pairing_signal.subscribe()
    }

    /// Stores a PC that has just paired. Pairing mode is the owner's deliberate act, so the PC is
    /// allowed even if it had been blocked; where it sits and how it streams are kept.
    pub fn pair_pc(&self, pc: Peer) -> Result<String, String> {
        let key = self.replace_peer(&pc.key(), pc)?;
        self.set_pc_allowed(&key, true)?;
        Ok(key)
    }

    pub fn set_pc_allowed(&self, key: &str, allowed: bool) -> Result<(), String> {
        if self.cfg.lock().unwrap().peer(key).is_none() {
            return Err("that PC is not in the list".to_string());
        }
        self.update(|cfg| {
            if let Some(pc) = cfg.peers.iter_mut().find(|p| p.key() == key) {
                pc.allowed = allowed;
            }
        })
    }

    /// Removes a PC. True when no PC is left (the stream host's own pairing should go too).
    pub fn forget_pc(&self, key: &str) -> Result<bool, String> {
        if self.cfg.lock().unwrap().peer(key).is_none() {
            return Err("that PC is not in the list".to_string());
        }
        self.update(|cfg| {
            cfg.remove_peer(key);
        })?;
        Ok(self.cfg.lock().unwrap().peers.is_empty())
    }

    /// The PC told the Mac which side of it the Mac sits on.
    pub fn set_pc_side(&self, key: &str, side: Side) -> Result<(), String> {
        if self.cfg.lock().unwrap().peer(key).is_none() {
            return Err("that PC is not in the list".to_string());
        }
        self.update(|cfg| {
            if let Some(pc) = cfg.peers.iter_mut().find(|p| p.key() == key) {
                pc.placement.side = side;
            }
        })
    }

    /// A paired PC proved itself from `ip`: it becomes the PC's address, and the one it had is
    /// kept among the others to try. Nothing is written when the address is the same.
    pub fn saw_pc_at(&self, key: &str, ip: std::net::IpAddr) {
        let ip = ip.to_string();
        let changed = {
            let cfg = self.cfg.lock().unwrap();
            cfg.peer(key).is_some_and(|p| p.host != ip)
        };
        if changed {
            let _ = self.update(|cfg| {
                if let Some(pc) = cfg.peer_mut(key) {
                    moved_to(pc, &ip);
                }
            });
        }
    }

    pub fn set_stay_awake(&self, enabled: bool) -> Result<(), String> {
        self.update(|cfg| cfg.stay_awake = enabled)
    }

    /// A Restart of `pc` was asked for (Mac side).
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fn note_restart(&self, pc: &str, now: Instant) {
        *self.restart.lock().unwrap() = Some(Restart { pc: pc.to_string(), at: now, cut: false });
    }

    /// The live restart record, dropping one older than the window.
    fn live_restart<'a>(restart: &'a mut Option<Restart>, now: Instant) -> Option<&'a mut Restart> {
        if restart.as_ref().is_some_and(|r| now.saturating_duration_since(r.at) > RESTART_CUT_WINDOW) {
            *restart = None;
        }
        restart.as_mut()
    }

    /// The PC being restarted, if a Restart is under way.
    pub fn restarting_pc(&self, now: Instant) -> Option<String> {
        let mut restart = self.restart.lock().unwrap();
        Self::live_restart(&mut restart, now).map(|r| r.pc.clone())
    }

    /// A Disconnect of `pc`: if it is being restarted, it is cut again when it is back, so the
    /// user's Disconnect wins over the restart. Whether it was.
    pub fn cut_when_back(&self, pc: &str, now: Instant) -> bool {
        let mut restart = self.restart.lock().unwrap();
        match Self::live_restart(&mut restart, now) {
            Some(r) if r.pc == pc => {
                r.cut = true;
                true
            }
            _ => false,
        }
    }

    /// `pc` connected again: ends its restart, and says whether a Disconnect came meanwhile and it
    /// must be cut now.
    pub fn back_from_restart(&self, pc: &str, now: Instant) -> bool {
        let mut restart = self.restart.lock().unwrap();
        match Self::live_restart(&mut restart, now) {
            Some(r) if r.pc == pc => restart.take().is_some_and(|r| r.cut),
            _ => false,
        }
    }

    /// The PC connected on the sharing channel (Mac side).
    pub fn connected_pc(&self) -> Option<String> {
        match &*self.mouse_status.lock().unwrap() {
            MouseStatus::Connected { mac_id, .. } => Some(mac_id.clone()),
            _ => None,
        }
    }

    pub fn set_display_size(&self, size: Option<DisplayDto>) {
        *self.display_size.lock().unwrap() = size;
    }

    /// The Stay awake setting itself, apart from whether a PC is connected.
    pub fn stay_awake(&self) -> bool {
        self.cfg.lock().unwrap().stay_awake
    }

    /// Whether the Mac should be kept from sleeping: a PC is connected and the setting is on.
    pub fn wants_awake(&self) -> bool {
        self.connected_pc().is_some() && self.cfg.lock().unwrap().stay_awake
    }

    /// Whether the PC calling as `pc_id` may start the display now: the PC connected on the
    /// sharing channel, or any PC while no other PC is (its sharing connection may be coming back
    /// up, or the sharing server cannot run), and only while nobody holds the display. A yes
    /// claims it until [`AppState::display_released`].
    pub fn may_start_display(&self, pc_id: &str) -> bool {
        if self.connected_pc().is_some_and(|other| other != pc_id) {
            return false;
        }
        let mut holder = self.display_holder.lock().unwrap();
        if holder.is_some() {
            return false;
        }
        *holder = Some(pc_id.to_string());
        true
    }

    pub fn display_released(&self, pc_id: &str) {
        let mut holder = self.display_holder.lock().unwrap();
        if holder.as_deref() == Some(pc_id) {
            *holder = None;
        }
    }

    /// Records a PC's display connection so it can be cut (blocking, forgetting, disconnecting
    /// it). The number identifies this registration for [`AppState::unregister_control`].
    pub fn register_control(&self, pc_id: &str) -> (CancellationToken, u64) {
        let mut controls = self.controls.lock().unwrap();
        let seq = self.control_seq.fetch_add(1, Ordering::SeqCst);
        let token = CancellationToken::new();
        controls.insert(pc_id.to_string(), (seq, token.clone()));
        (token, seq)
    }

    pub fn unregister_control(&self, pc_id: &str, seq: u64) {
        let mut controls = self.controls.lock().unwrap();
        if controls.get(pc_id).is_some_and(|(n, _)| *n == seq) {
            controls.remove(pc_id);
        }
    }

    /// Ends the PC's display connection, if it has one.
    pub fn kick_control(&self, pc_id: &str) {
        if let Some((_, token)) = self.controls.lock().unwrap().get(pc_id) {
            token.cancel();
        }
    }

    pub fn forget_peer(&self) -> Result<(), String> {
        if !self.is_idle() {
            return Err("disconnect first".to_string());
        }
        self.update(|cfg| {
            if let Some(key) = cfg.current.clone() {
                cfg.remove_peer(&key);
            }
        })
    }

    /// Replaces a damaged settings file with fresh defaults.
    pub fn reset_config(&self) -> Result<(), String> {
        *self.config_error.lock().unwrap() = None;
        self.update(|cfg| *cfg = Config::default_new())
    }

    pub fn status(&self) -> StatusDto {
        let mouse = self.mouse_status.lock().unwrap().clone();
        let cfg = self.cfg.lock().unwrap();
        let peer = cfg.current_peer();
        let stream = peer.map(|p| p.stream.clone()).unwrap_or_default();
        let connection = match &mouse {
            MouseStatus::Off => "off",
            MouseStatus::Connecting => "connecting",
            MouseStatus::Connected { .. } => "connected",
            MouseStatus::Lost(_) => "lost",
            MouseStatus::Ended(EndReason::ByMac) => "ended_by_mac",
            MouseStatus::Ended(EndReason::Busy { .. }) => "busy",
            MouseStatus::Ended(EndReason::Blocked) => "blocked",
            MouseStatus::Ended(EndReason::Unknown) => "unknown",
            MouseStatus::Ended(EndReason::Version) => "version",
        };
        let busy_by = match &mouse {
            MouseStatus::Ended(EndReason::Busy { by }) => Some(by.clone()),
            _ => None,
        };
        let connected_id = match &mouse {
            MouseStatus::Connected { mac_id, .. } => Some(mac_id.clone()),
            _ => None,
        };
        let uses = match &mouse {
            MouseStatus::Connected { uses: Some(Uses { mouse, clipboard, display, .. }), .. } => {
                Some(UsesDto { mouse: *mouse, clipboard: *clipboard, display: *display })
            }
            _ => None,
        };
        let latency_ms = match &mouse {
            MouseStatus::Connected { rtt_ms, .. } => *rtt_ms,
            _ => None,
        };
        let side_word = |side: Side| match side {
            Side::Left => "left",
            Side::Right => "right",
            Side::Top => "top",
            Side::Bottom => "bottom",
        };
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
            peer_name: peer.map(|p| p.name.clone()),
            peer_host: peer.map(|p| p.host.clone()),
            paired: peer.is_some_and(|p| !p.token.is_empty()),
            settings: SettingsDto {
                width: stream.width,
                height: stream.height,
                fps: stream.fps,
                bitrate_mbps: stream.bitrate_kbps / 1000,
                window_mode: match stream.window_mode {
                    WindowMode::Windowed => "windowed".to_string(),
                    WindowMode::Borderless => "borderless".to_string(),
                },
                monitor: stream.monitor,
            },
            config_error: self.config_error.lock().unwrap().clone(),
            mouse_sharing: cfg.mouse_sharing,
            mac_side: side_word(peer.map(|p| p.placement.side).unwrap_or(Side::Right)),
            mouse_state: match &mouse {
                MouseStatus::Off => "off",
                // Connected, but the PC's input cannot be replayed: the connection stands.
                MouseStatus::Connected { problem: Some(_), .. } => "failed",
                MouseStatus::Connecting | MouseStatus::Connected { .. } => "running",
                MouseStatus::Lost(_) | MouseStatus::Ended(_) => "failed",
            },
            mouse_reason: match mouse {
                MouseStatus::Lost(reason) => Some(reason),
                MouseStatus::Connected { problem: Some(problem), .. } => Some(problem),
                MouseStatus::Ended(why) => Some(match why {
                    EndReason::ByMac => "the Mac ended the connection".to_string(),
                    EndReason::Busy { by } => format!("the Mac is in use by {by}"),
                    EndReason::Blocked => "the Mac has blocked this PC".to_string(),
                    EndReason::Unknown => "the Mac does not know this PC".to_string(),
                    EndReason::Version => "the other computer runs another version of Expand; update it".to_string(),
                }),
                _ => None,
            },
            connection,
            busy_by,
            peers: cfg
                .peers
                .iter()
                .map(|p| PeerDto {
                    key: p.key(),
                    name: p.name.clone(),
                    host: p.host.clone(),
                    addresses: p.addresses.clone(),
                    current: cfg.current.as_deref() == Some(p.key().as_str()),
                    allowed: p.allowed,
                    paired: !p.token.is_empty(),
                    connected: !p.id.is_empty() && connected_id.as_deref() == Some(p.id.as_str()),
                })
                .collect(),
            auto_connect: cfg.auto_connect,
            placement: PlacementDto {
                side: side_word(peer.map(|p| p.placement.side).unwrap_or(Side::Right)),
                monitor: peer.and_then(|p| p.placement.monitor),
            },
            uses,
            pairing: self.pairing_open().map(|(code, seconds_left)| PairingDto { code, seconds_left }),
            close_to_tray: cfg.close_to_tray,
            clipboard_sharing: cfg.clipboard_sharing,
            language_follow: cfg.language_follow,
            start_display_on_connect: cfg.start_display_on_connect,
            ctrl_as_cmd: cfg.ctrl_as_cmd,
            reverse_scroll: cfg.reverse_scroll,
            mac_sound: cfg.mac_sound,
            peer_addresses: peer.map(|p| p.addresses.clone()).unwrap_or_default(),
            latency_ms,
            viewer_monitor: viewer_monitor_number(peer),
            display: self.display_size.lock().unwrap().clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use expand_core::config::{Config, StreamSettings, WindowMode};
    #[allow(unused_imports)] // only the PC's tests choose a side
    use expand_core::side::Side;
    use expand_core::mouse::MouseStatus;
    use expand_core::session::State;

    fn app() -> (AppState, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        (AppState::new(Config::default_new(), path, None), dir)
    }

    fn settings(width: u32, fps: u32, bitrate_kbps: u32) -> StreamSettings {
        StreamSettings { width, height: 1080, fps, bitrate_kbps, window_mode: WindowMode::Borderless, monitor: None }
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
    fn changing_settings_while_running_is_saved_not_refused() {
        let (a, _d) = app();
        a.set_peer_host("192.168.1.20").unwrap();
        a.state.send_replace(State::Connected);
        a.save_settings(settings(1920, 60, 20000)).unwrap();
        a.state.send_replace(State::Connecting);
        a.save_settings(settings(2560, 120, 50000)).unwrap();
        assert_eq!(Config::load(&a.path).unwrap().current_peer().unwrap().stream, settings(2560, 120, 50000));
    }

    #[test]
    fn save_settings_persists_when_idle() {
        let (a, _d) = app();
        a.set_peer_host("192.168.1.20").unwrap();
        a.save_settings(settings(1920, 60, 20000)).unwrap();
        assert_eq!(Config::load(&a.path).unwrap().current_peer().unwrap().stream, settings(1920, 60, 20000));
        a.state.send_replace(State::Failed { part: "x".into(), reason: "y".into() });
        a.save_settings(settings(2560, 120, 50000)).unwrap();
    }

    #[test]
    fn the_display_windows_mode_is_saved_alone_and_the_display_keeps_running() {
        let (a, _d) = app();
        a.set_peer_host("192.168.1.20").unwrap();
        let windowed = StreamSettings { window_mode: WindowMode::Windowed, ..settings(1920, 60, 20000) };
        a.save_settings(windowed.clone()).unwrap();
        let token = a.begin_session().unwrap();
        a.save_display_mode(WindowMode::Borderless, Some(2)).unwrap();
        let saved = || Config::load(&a.path).unwrap().current_peer().unwrap().stream.clone();
        assert_eq!(saved(), StreamSettings { window_mode: WindowMode::Borderless, monitor: Some(2), ..windowed.clone() });
        // Back to a window: the monitor stays as it was.
        a.save_display_mode(WindowMode::Windowed, None).unwrap();
        assert_eq!(saved(), StreamSettings { monitor: Some(2), ..windowed });
        assert!(!token.is_cancelled());
        assert!(a.has_session());
    }

    #[test]
    fn the_display_windows_mode_needs_a_mac() {
        let (a, _d) = app();
        assert!(a.save_display_mode(WindowMode::Borderless, Some(1)).is_err());
    }

    #[test]
    fn save_settings_rejects_values_out_of_range() {
        let (a, _d) = app();
        a.set_peer_host("192.168.1.20").unwrap();
        assert!(a.save_settings(settings(100, 60, 20000)).is_err());
        assert!(a.save_settings(settings(1920, 0, 20000)).is_err());
        assert!(a.save_settings(settings(1920, 60, 500)).is_err());
        assert!(a.save_settings(settings(1920, 60, 500_000)).is_err());
    }

    fn paired_mac() -> Peer {
        Peer {
            name: "Studio".into(),
            id: "mac-1".into(),
            host: "192.168.1.50".into(),
            token: "secret".into(),
            sunshine_paired: true,
            addresses: vec!["100.64.0.2".into()],
            placement: Default::default(),
            stream: Default::default(),
            allowed: true,
        }
    }

    #[test]
    fn editing_a_macs_address_keeps_its_pairing() {
        let (a, _d) = app();
        a.update(|cfg| {
            let key = cfg.upsert_peer(paired_mac());
            cfg.current = Some(key);
        })
        .unwrap();
        assert!(a.set_mac_address("mac-1", "not an address").is_err());
        assert!(a.set_mac_address("nobody", "192.168.1.60").is_err());
        assert_eq!(a.set_mac_address("mac-1", "192.168.1.50").unwrap(), None, "same address: nothing to do");
        let was_current = a.set_mac_address("mac-1", " 192.168.1.60 ").unwrap();
        assert_eq!(was_current, Some(true));
        let saved = Config::load(&a.path).unwrap();
        let peer = saved.peer("mac-1").unwrap();
        assert_eq!(peer.host, "192.168.1.60");
        assert_eq!(peer.addresses, vec!["192.168.1.50".to_string(), "100.64.0.2".to_string()]);
        assert_eq!(peer.token, "secret");
        assert!(peer.sunshine_paired);
        assert_eq!(saved.current.as_deref(), Some("mac-1"));
    }

    // Plan 2 final review, item 6: an Enter on a half-typed address stopped the display before
    // the address was found wrong. Every refusal now comes first, with nothing stopped.
    #[tokio::test]
    async fn a_refused_address_leaves_the_display_running() {
        let (a, _d) = app();
        a.update(|cfg| {
            let key = cfg.upsert_peer(paired_mac());
            cfg.current = Some(key);
        })
        .unwrap();
        let other = a.add_peer_by_host("192.168.1.70").unwrap();
        let display = a.begin_session().unwrap();
        assert!(a.change_mac_address("mac-1", "192.168.1.").await.is_err());
        assert!(a.change_mac_address("nobody", "192.168.1.60").await.is_err());
        assert!(a.change_mac_address("mac-1", "192.168.1.70").await.is_err());
        assert!(!display.is_cancelled());
        assert_eq!(Config::load(&a.path).unwrap().peer("mac-1").unwrap().host, "192.168.1.50");

        // The same address: nothing to change, nothing stopped; the key to connect to.
        assert_eq!(a.change_mac_address("mac-1", " 192.168.1.50 ").await.unwrap(), (None, "mac-1".to_string()));
        // Another Mac than the one on display: its display runs on. A Mac known by address alone
        // gets a new key with its new address.
        assert_eq!(a.change_mac_address(&other, "192.168.1.71").await.unwrap(), (Some(false), "host:192.168.1.71".to_string()));
        assert!(!display.is_cancelled());

        // The current Mac's own display stops for its new address.
        let ended = std::sync::Arc::new(a);
        let watcher = ended.clone();
        tokio::spawn(async move {
            display.cancelled().await;
            watcher.end_session();
        });
        assert_eq!(ended.change_mac_address("mac-1", "192.168.1.60").await.unwrap(), (Some(true), "mac-1".to_string()));
        assert!(!ended.has_session());
    }

    #[test]
    fn an_address_used_by_another_mac_is_refused() {
        let (a, _d) = app();
        a.update(|cfg| {
            let key = cfg.upsert_peer(paired_mac());
            cfg.current = Some(key);
        })
        .unwrap();
        let a1 = a.add_peer_by_host("192.168.1.70").unwrap();
        a.add_peer_by_host("192.168.1.71").unwrap();
        let refused = "That address belongs to another Mac";
        assert_eq!(a.set_mac_address(&a1, "192.168.1.71").unwrap_err(), refused);
        assert_eq!(a.set_mac_address("mac-1", "192.168.1.71").unwrap_err(), refused);
        assert_eq!(Config::load(&a.path).unwrap().peers.len(), 3);
        assert!(a.set_mac_address(&a1, "192.168.1.72").is_ok());
    }

    #[tokio::test]
    async fn reconnect_without_a_mac_says_choose_a_mac() {
        let (a, _d) = app();
        let err = a.connect_to(None, None, 1).await.unwrap_err();
        assert_eq!(err, "choose a Mac first");
    }

    // Plan 2 final review, item 2: Reconnect re-runs the connection without disconnecting. Its
    // state part (the chosen Mac checked, the connection wanted) leaves the display alone.
    #[tokio::test]
    async fn reconnecting_leaves_a_running_display_alone() {
        let (a, _d) = app();
        paired(&a);
        let display = a.begin_session().unwrap();
        a.set_want_connection(false);
        a.connect_to(None, None, 1).await.unwrap();
        assert!(!display.is_cancelled());
        assert!(a.has_session());
        assert!(a.want_connection());
    }

    #[test]
    fn the_status_carries_the_round_trip() {
        let (a, _d) = app();
        assert_eq!(a.status().latency_ms, None);
        a.set_mouse_status(MouseStatus::Connected { mac_id: "m".into(), mac_name: "M".into(), uses: None, problem: None, from: None, rtt_ms: Some(3) });
        assert_eq!(a.status().latency_ms, Some(3));
    }

    #[test]
    fn set_peer_host_needs_an_ip_address_and_starts_unpaired() {
        let (a, _d) = app();
        assert!(a.set_peer_host("not an address").is_err());
        a.set_peer_host(" 192.168.1.50 ").unwrap();
        let peer = Config::load(&a.path).unwrap().current_peer().cloned().unwrap();
        assert_eq!(peer.host, "192.168.1.50");
        assert!(peer.token.is_empty());
        assert!(!peer.sunshine_paired);
    }

    #[test]
    fn the_side_belongs_to_the_chosen_mac_and_survives_a_new_address() {
        let (a, _d) = app();
        assert!(a.set_placement(Placement { side: Side::Top, monitor: None }).is_err(), "no Mac chosen yet");
        a.set_peer_host("192.168.1.20").unwrap();
        a.set_placement(Placement { side: Side::Top, monitor: None }).unwrap();
        a.set_peer_host("192.168.1.21").unwrap();
        let cfg = Config::load(&a.path).unwrap();
        assert_eq!(cfg.peers.len(), 1);
        assert_eq!(cfg.current_peer().unwrap().placement.side, Side::Top);
    }

    #[test]
    fn forget_peer_clears_it() {
        let (a, _d) = app();
        a.set_peer_host("192.168.1.50").unwrap();
        a.forget_peer().unwrap();
        assert!(Config::load(&a.path).unwrap().peers.is_empty());
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
            placement: Default::default(),
            stream: Default::default(),
            allowed: true,
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
        use expand_core::mouse::MouseRole;
        let (a, _d) = app();
        paired(&a);
        a.set_placement(Placement { side: Side::Top, monitor: None }).unwrap();
        let MouseRole::Pc(s) = a.mouse_role().unwrap();
        assert_eq!(s.macs.iter().map(|m| m.to_string()).collect::<Vec<_>>(), vec!["192.168.1.50"]);
        assert_eq!(s.token, "t".repeat(64));
        assert_eq!(s.my_id, a.cfg.lock().unwrap().device_id);
        assert_eq!(s.placement.side, Side::Top);
        assert_eq!(s.stream, (2560, 1440));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_mac_shares_with_the_trusted_pc() {
        use expand_core::mouse::MouseRole;
        let (a, _d) = app();
        paired(&a);
        let MouseRole::Mac(s) = a.mouse_role().unwrap();
        let pcs = s.pcs.borrow().clone();
        assert_eq!((pcs[0].0.as_str(), pcs[0].1.len(), pcs[0].2), ("macid", 64, true));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_mac_serves_paired_pcs_whichever_is_current() {
        use expand_core::mouse::MouseRole;
        let (a, _d) = app();
        assert!(a.mouse_role().is_none(), "nobody is paired");
        paired(&a);
        a.update(|cfg| {
            cfg.current = None;
            let mut stranger = cfg.peers[0].clone();
            stranger.id = "unpaired".into();
            stranger.token.clear();
            cfg.peers.push(stranger);
        })
        .unwrap();
        let MouseRole::Mac(s) = a.mouse_role().expect("listens without a current peer");
        assert_eq!(s.pcs.borrow().len(), 1, "a peer without a token is never let in");
        a.update(|cfg| cfg.peers[0].allowed = false).unwrap();
        assert!(a.mouse_role().is_none(), "only blocked PCs: nothing to serve");
    }

    #[test]
    fn the_mac_lets_in_paired_pcs_only() {
        let mut cfg = Config::default_new();
        cfg.upsert_peer(a_pc("pc-1", &token('a'), true));
        cfg.upsert_peer(a_pc("pc-2", "", true));
        cfg.upsert_peer(a_pc("pc-3", &token('c'), false));
        let ids: Vec<_> = known_pcs(&cfg).into_iter().map(|p| (p.0, p.2)).collect();
        assert_eq!(ids, vec![("pc-1".to_string(), true), ("pc-3".to_string(), false)]);
    }

    // Final review C1: pairing, blocking or forgetting another PC restarted the Mac's server and
    // dropped the connected PC. The role stays the same; the running server gets the new list.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_change_to_the_pc_list_reaches_the_running_server_without_a_restart() {
        let (a, _d) = app();
        a.pair_pc(a_pc("pc-1", &token('a'), true)).unwrap();
        let before = a.mouse_role().unwrap();
        let MouseRole::Mac(running) = before.clone();
        let mut seen = running.pcs.clone();
        seen.borrow_and_update();
        a.pair_pc(a_pc("pc-2", &token('b'), true)).unwrap();
        assert_eq!(a.mouse_role().unwrap(), before, "the same role: no restart");
        assert!(seen.has_changed().unwrap());
        assert_eq!(seen.borrow_and_update().len(), 2);
        a.set_pc_allowed("pc-1", false).unwrap();
        assert_eq!(a.mouse_role().unwrap(), before);
        assert!(!seen.borrow_and_update().iter().find(|p| p.0 == "pc-1").unwrap().2);
        a.set_stay_awake(false).unwrap();
        assert!(!seen.has_changed().unwrap(), "nothing about the PCs changed");
    }

    #[test]
    fn switching_mouse_and_clipboard_sharing_off_is_saved() {
        let (a, _d) = app();
        paired(&a);
        a.set_mouse_sharing(false).unwrap();
        a.set_clipboard_sharing(false).unwrap();
        assert!(a.mouse_role().is_some(), "the connection stays; only what is shared changes");
        assert!(!Config::load(&a.path).unwrap().mouse_sharing);
        assert!(!a.status().mouse_sharing);
    }

    // Final review (ledger minor): the tray's Disconnect ended only the display, and was not
    // offered while only the connection was up. Now: Connect when not connected, Disconnect
    // whenever connected.
    #[test]
    fn the_tray_offers_disconnect_whenever_connected() {
        let (a, _d) = app();
        paired(&a);
        let ids = |a: &AppState| {
            client_tray_actions(&a.status()).into_iter().filter(|t| t.checked.is_none()).map(|t| (t.id, t.label)).collect::<Vec<_>>()
        };
        assert_eq!(ids(&a), vec![("connect", "Connect")]);
        a.set_mouse_status(MouseStatus::Connected { mac_id: "macid".into(), mac_name: "MacBook".into(), uses: None, problem: None, from: None, rtt_ms: None });
        assert_eq!(ids(&a), vec![("disconnect", "Disconnect")]);
        a.state.send_replace(State::Connected);
        assert_eq!(ids(&a), vec![("disconnect", "Disconnect")]);
    }

    #[test]
    fn the_tray_toggles_show_their_state() {
        let (a, _d) = app();
        paired(&a);
        let toggles = |a: &AppState| {
            client_tray_actions(&a.status()).into_iter().filter(|t| t.checked.is_some()).map(|t| (t.id, t.label, t.enabled, t.checked.unwrap())).collect::<Vec<_>>()
        };
        a.set_mouse_sharing(false).unwrap();
        assert_eq!(toggles(&a), vec![("mouse", "Mouse and keyboard", true, false), ("display", "Extended display", false, false)]);
        a.set_mouse_sharing(true).unwrap();
        a.set_mouse_status(MouseStatus::Connected { mac_id: "macid".into(), mac_name: "MacBook".into(), uses: None, problem: None, from: None, rtt_ms: None });
        assert_eq!(toggles(&a), vec![("mouse", "Mouse and keyboard", true, true), ("display", "Extended display", true, false)]);
        a.state.send_replace(State::Connected);
        assert_eq!(toggles(&a)[1], ("display", "Extended display", true, true));
    }

    // Plan 2 final review, item 1: the link went `lost` while the stream kept running, and the
    // tray's Extended display was disabled, so nothing could stop the display.
    #[test]
    fn the_tray_can_stop_a_running_display_whatever_the_link_does() {
        let (a, _d) = app();
        paired(&a);
        let display = |a: &AppState| {
            client_tray_actions(&a.status()).into_iter().find(|t| t.id == "display").map(|t| (t.enabled, t.checked.unwrap())).unwrap()
        };
        a.set_mouse_status(MouseStatus::Lost("the network went away".into()));
        assert_eq!(display(&a), (false, false), "no display, no link: nothing to start");
        a.state.send_replace(State::Connected);
        assert_eq!(display(&a), (true, true), "running: checked and enabled to stop it");
        a.state.send_replace(State::Connecting);
        assert_eq!(display(&a), (true, true), "starting: can be stopped too");
        a.set_mouse_status(MouseStatus::Off);
        assert_eq!(display(&a), (true, true));
    }

    // 2026-11-18: mouse sharing keeps running while the display is disconnected, and that must
    // be visible, not a surprise.
    #[test]
    fn the_tray_says_when_mouse_sharing_is_on() {
        assert_eq!(tray_text("disconnected", "running"), "Expand: disconnected · mouse sharing on");
        assert_eq!(tray_text("connected", "running"), "Expand: connected · mouse sharing on");
        assert_eq!(tray_text("disconnected", "off"), "Expand: disconnected");
        assert_eq!(tray_text("connected", "failed"), "Expand: connected · mouse sharing not working");
    }

    #[cfg(windows)]
    #[test]
    fn clipboard_and_language_are_on_by_default_and_reach_sharing() {
        use expand_core::mouse::MouseRole;
        let (a, _d) = app();
        paired(&a);
        let s = a.status();
        assert_eq!((s.clipboard_sharing, s.language_follow), (true, true));
        let MouseRole::Pc(p) = a.mouse_role().unwrap();
        assert!(p.toggles.mouse && p.toggles.clipboard && p.toggles.language);
        a.set_clipboard_sharing(false).unwrap();
        a.set_language_follow(false).unwrap();
        let MouseRole::Pc(p) = a.mouse_role().unwrap();
        assert!(!p.toggles.clipboard && !p.toggles.language);
        assert!(!Config::load(&a.path).unwrap().clipboard_sharing);
    }

    #[cfg(windows)]
    #[test]
    fn ctrl_as_cmd_is_on_by_default_and_reaches_sharing() {
        use expand_core::mouse::MouseRole;
        let (a, _d) = app();
        paired(&a);
        assert!(a.status().ctrl_as_cmd);
        let MouseRole::Pc(p) = a.mouse_role().unwrap();
        assert!(p.toggles.ctrl_as_cmd);
        a.set_ctrl_as_cmd(false).unwrap();
        let MouseRole::Pc(p) = a.mouse_role().unwrap();
        assert!(!p.toggles.ctrl_as_cmd);
        assert!(!a.status().ctrl_as_cmd);
        assert!(!Config::load(&a.path).unwrap().ctrl_as_cmd);
    }

    #[cfg(windows)]
    #[test]
    fn reverse_scroll_is_on_by_default_and_reaches_sharing() {
        use expand_core::mouse::MouseRole;
        let (a, _d) = app();
        paired(&a);
        assert!(a.status().reverse_scroll);
        let MouseRole::Pc(p) = a.mouse_role().unwrap();
        assert!(p.toggles.reverse_scroll);
        a.set_reverse_scroll(false).unwrap();
        let MouseRole::Pc(p) = a.mouse_role().unwrap();
        assert!(!p.toggles.reverse_scroll);
        assert!(!a.status().reverse_scroll);
        assert!(!Config::load(&a.path).unwrap().reverse_scroll);
        a.set_reverse_scroll(true).unwrap();
    }

    /// Mac sound is on by default, is saved, and is what the next display starts with, as is
    /// Reverse scrolling.
    #[test]
    fn mac_sound_is_on_by_default_and_reaches_the_next_display() {
        use expand_core::moonlight::Prefs;
        let (a, _d) = app();
        assert!(a.status().mac_sound);
        assert_eq!(a.stream_prefs(), Prefs { reverse_scroll: true, sound_here: true });
        a.set_mac_sound(false).unwrap();
        assert!(!a.status().mac_sound);
        assert!(!Config::load(&a.path).unwrap().mac_sound);
        a.set_reverse_scroll(false).unwrap();
        assert_eq!(a.stream_prefs(), Prefs { reverse_scroll: false, sound_here: false });
    }

    fn connected() -> MouseStatus {
        MouseStatus::Connected { mac_id: "m".into(), mac_name: "Mac".into(), uses: None, problem: None, from: None, rtt_ms: None }
    }

    /// Runs the statuses through, as the watcher sees them; whether each started the display.
    fn starts(a: &AppState, statuses: &[MouseStatus]) -> Vec<bool> {
        statuses.iter().map(|s| a.starts_display_now(s)).collect()
    }

    fn lost() -> MouseStatus {
        MouseStatus::Lost("the Mac stopped answering".into())
    }

    fn starting_on_connect() -> (AppState, tempfile::TempDir) {
        let (a, d) = app();
        a.set_start_display_on_connect(true).unwrap();
        (a, d)
    }

    #[test]
    fn the_display_starts_when_the_connection_the_user_asked_for_comes_up() {
        let (a, _d) = starting_on_connect();
        a.ask_connect(true);
        assert_eq!(starts(&a, &[MouseStatus::Connecting, connected()]), [false, true]);
        // The Mac answered late.
        a.ask_connect(true);
        assert_eq!(starts(&a, &[MouseStatus::Connecting, lost(), MouseStatus::Off, connected()]), [false, false, false, true]);
    }

    #[test]
    fn the_connection_restarting_or_coming_back_by_itself_starts_nothing() {
        let (a, _d) = starting_on_connect();
        a.ask_connect(true);
        assert_eq!(starts(&a, &[connected()]), [true]);
        // A new address list restarts the link; a blip loses it; the Mac's Restart: none was asked.
        assert_eq!(starts(&a, &[MouseStatus::Off, MouseStatus::Connecting, connected()]), [false, false, false]);
        assert_eq!(starts(&a, &[lost(), connected()]), [false, false]);
        // A round-trip update while connected neither.
        let faster = MouseStatus::Connected { mac_id: "m".into(), mac_name: "Mac".into(), uses: None, problem: None, from: None, rtt_ms: Some(3) };
        assert_eq!(starts(&a, &[faster]), [false]);
    }

    #[test]
    fn a_refused_connection_drops_the_ask() {
        let (a, _d) = starting_on_connect();
        a.ask_connect(true);
        let refused = MouseStatus::Ended(expand_core::mouse::EndReason::Blocked);
        assert_eq!(starts(&a, &[refused, connected()]), [false, false]);
    }

    #[cfg(windows)]
    #[test]
    fn the_launch_asks_to_connect_only_when_a_connection_will_run() {
        // No paired Mac: nothing will connect, so nothing is asked.
        let (a, _d) = starting_on_connect();
        a.ask_connect_at_launch();
        assert_eq!(starts(&a, &[connected()]), [false]);
        // A paired Mac, with the connection wanted (auto-connect).
        paired(&a);
        a.ask_connect_at_launch();
        assert_eq!(starts(&a, &[connected()]), [true]);
        // Auto-connect off: not wanted, nothing asked.
        a.set_want_connection(false);
        a.ask_connect_at_launch();
        assert_eq!(starts(&a, &[connected()]), [false]);
    }

    #[cfg(windows)]
    #[test]
    fn a_connection_no_longer_wanted_drops_the_ask() {
        // Asked, then disconnected (or the Mac forgotten) before it came up: a later link-up
        // must not open the display.
        let (a, _d) = starting_on_connect();
        paired(&a);
        a.ask_connect(true);
        a.set_want_connection(false);
        assert!(a.connection_role().is_none());
        a.set_want_connection(true);
        assert!(a.connection_role().is_some());
        assert_eq!(starts(&a, &[connected()]), [false]);
    }

    /// The Mac slept (or left the network) under a running display: the display comes back,
    /// once, as the connection does.
    #[test]
    fn a_display_that_ended_by_itself_comes_back_with_the_connection() {
        let (a, _d) = app();
        let _token = a.begin_session().unwrap();
        a.set_mouse_status(lost());
        a.end_session();
        assert_eq!(a.display_ended(false, &lost()), Comeback::WhenConnected);
        assert!(!a.resumes_display_now(false, &lost()), "not while the Mac is away");
        assert!(a.resumes_display_now(false, &connected()));
        assert!(!a.resumes_display_now(false, &connected()), "once");
    }

    /// The connection dropped and came back before the stream noticed: the display starts again
    /// at once.
    #[test]
    fn a_display_that_ended_after_the_connection_came_back_starts_again_at_once() {
        let (a, _d) = app();
        let _token = a.begin_session().unwrap();
        a.set_mouse_status(lost());
        a.set_mouse_status(connected());
        a.end_session();
        assert_eq!(a.display_ended(false, &connected()), Comeback::Now);
    }

    /// Closed by the user, it stays closed; failed on a connection that never dropped, it shows
    /// why (with Retry) instead of starting over and over.
    #[test]
    fn a_display_stopped_or_failing_on_a_steady_connection_does_not_come_back() {
        let (a, _d) = app();
        let _token = a.begin_session().unwrap();
        a.set_mouse_status(lost());
        a.end_session();
        assert_eq!(a.display_ended(true, &lost()), Comeback::No);
        assert!(!a.resumes_display_now(false, &connected()));

        let _token = a.begin_session().unwrap();
        a.set_mouse_status(connected());
        a.end_session();
        assert_eq!(a.display_ended(false, &connected()), Comeback::No);
    }

    /// Waiting to come back, it is called off by the user closing the display, and never fires
    /// on a connection that is already up (its round trip updates come as Connected again).
    #[test]
    fn a_display_waiting_to_come_back_is_called_off_and_needs_the_connection_to_return() {
        let (a, _d) = app();
        let _token = a.begin_session().unwrap();
        a.set_mouse_status(lost());
        a.end_session();
        assert_eq!(a.display_ended(false, &lost()), Comeback::WhenConnected);
        assert!(!a.resumes_display_now(true, &connected()), "the connection was up already");
        a.call_off_comeback();
        assert!(!a.resumes_display_now(false, &connected()));
    }

    #[test]
    fn the_display_does_not_start_when_off_or_already_running() {
        let (a, _d) = app();
        a.set_start_display_on_connect(false).unwrap();
        a.ask_connect(true);
        assert_eq!(starts(&a, &[connected()]), [false], "the option is off");
        assert!(!starts_display_on_connect(true, &connected(), true, true), "a display runs already");
        assert!(!starts_display_on_connect(true, &MouseStatus::Connecting, true, false));
    }

    #[test]
    fn starting_the_display_on_connect_is_on_by_default_and_saved() {
        let (a, _d) = app();
        assert!(a.status().start_display_on_connect);
        a.set_start_display_on_connect(false).unwrap();
        assert!(!a.status().start_display_on_connect);
        assert!(!Config::load(&a.path).unwrap().start_display_on_connect);
    }

    // The connection to the Mac stays up whatever is shared; mouse off stops everything on it.
    #[cfg(windows)]
    #[test]
    fn mouse_off_also_turns_off_clipboard_and_language_but_keeps_the_connection() {
        use expand_core::mouse::MouseRole;
        let (a, _d) = app();
        paired(&a);
        a.set_mouse_sharing(false).unwrap();
        let MouseRole::Pc(p) = a.mouse_role().expect("the connection stays");
        assert!(!p.toggles.mouse && !p.toggles.clipboard && !p.toggles.language);
    }

    fn without_auto_connect(a: &AppState) -> AppState {
        let mut cfg = a.cfg.lock().unwrap().clone();
        cfg.auto_connect = false;
        AppState::new(cfg, a.path.clone(), None)
    }

    #[test]
    fn without_auto_connect_nothing_connects_at_launch() {
        let (a, _d) = app();
        paired(&a);
        assert!(a.want_connection(), "auto connect is on by default");
        let b = without_auto_connect(&a);
        assert!(!b.want_connection());
        assert!(!b.status().auto_connect);
        #[cfg(windows)]
        assert!(b.mouse_role().is_none());
        b.set_want_connection(true);
        #[cfg(windows)]
        assert!(b.mouse_role().is_some());
    }

    #[test]
    fn disconnect_is_sticky_until_connect() {
        let (a, _d) = app();
        paired(&a);
        a.set_want_connection(false);
        #[cfg(windows)]
        assert!(a.mouse_role().is_none());
        a.set_mouse_status(MouseStatus::Lost("network".into()));
        a.set_mouse_status(MouseStatus::Off);
        assert!(!a.want_connection(), "nothing but connect turns it back on");
        a.set_want_connection(true);
        #[cfg(windows)]
        assert!(a.mouse_role().is_some());
    }

    #[test]
    fn being_refused_as_busy_stops_trying_and_shows_who() {
        let (a, _d) = app();
        paired(&a);
        a.set_mouse_status(MouseStatus::Ended(EndReason::Busy { by: "OTHER-PC".into() }));
        let s = a.status();
        assert_eq!((s.connection, s.busy_by.as_deref()), ("busy", Some("OTHER-PC")));
        assert!(!a.want_connection());
        #[cfg(windows)]
        assert!(a.mouse_role().is_none());
        for (end, word) in [
            (EndReason::ByMac, "ended_by_mac"),
            (EndReason::Blocked, "blocked"),
            (EndReason::Unknown, "unknown"),
            (EndReason::Version, "version"),
        ] {
            a.set_want_connection(true);
            a.set_mouse_status(MouseStatus::Ended(end));
            assert_eq!(a.status().connection, word);
            assert!(!a.want_connection());
        }
    }

    #[test]
    fn a_network_loss_keeps_trying() {
        let (a, _d) = app();
        paired(&a);
        a.set_mouse_status(MouseStatus::Lost("network".into()));
        assert_eq!(a.status().connection, "lost");
        assert!(a.want_connection());
        #[cfg(windows)]
        assert!(a.mouse_role().is_some());
    }

    #[test]
    fn connection_words_follow_the_mouse_status() {
        let (a, _d) = app();
        assert_eq!(a.status().connection, "off");
        a.set_mouse_status(MouseStatus::Connecting);
        assert_eq!(a.status().connection, "connecting");
        a.set_mouse_status(MouseStatus::Connected { mac_id: "macid".into(), mac_name: "MacBook".into(), uses: None, problem: None, from: None, rtt_ms: None });
        assert_eq!(a.status().connection, "connected");
    }

    #[test]
    fn old_pause_setting_is_ignored() {
        let (a, _d) = app();
        paired(&a);
        let mut text = std::fs::read_to_string(&a.path).unwrap();
        text.push_str("
sharing_paused = true
");
        std::fs::write(&a.path, text).unwrap();
        let b = AppState::new(Config::load(&a.path).unwrap(), a.path.clone(), None);
        assert!(b.want_connection());
        assert!(!b.status().peers.is_empty());
        #[cfg(windows)]
        assert!(b.mouse_role().is_some());
    }

    #[test]
    fn the_status_lists_the_macs_and_where_the_current_one_sits() {
        let (a, _d) = app();
        paired(&a);
        a.set_placement(Placement { side: Side::Top, monitor: None }).unwrap();
        let s = a.status();
        assert_eq!(s.peers.len(), 1);
        let p = &s.peers[0];
        assert_eq!((p.key.as_str(), p.name.as_str(), p.current, p.allowed, p.paired), ("macid", "MacBook", true, true, true));
        assert_eq!(s.placement.side, "top");
        assert!(s.uses.is_none() && s.pairing.is_none());
    }

    #[test]
    fn learning_the_macs_id_keeps_its_placement_and_settings() {
        let (a, _d) = app();
        a.set_peer_host("192.168.1.50").unwrap();
        a.set_placement(Placement { side: Side::Top, monitor: None }).unwrap();
        a.update(|cfg| {
            let p = cfg.current_peer_mut().unwrap();
            p.token = "t".repeat(64);
            p.stream.fps = 30;
        })
        .unwrap();
        a.learn_mac("macid", "MacBook");
        let cfg = Config::load(&a.path).unwrap();
        assert_eq!(cfg.peers.len(), 1);
        let p = cfg.current_peer().unwrap();
        assert_eq!((p.id.as_str(), p.name.as_str(), p.host.as_str()), ("macid", "MacBook", "192.168.1.50"));
        assert_eq!((p.placement.side, p.stream.fps, p.token.len()), (Side::Top, 30, 64));
        assert_eq!(cfg.current.as_deref(), Some("macid"));
    }

    fn mac_at(id: &str, host: &str, token: &str) -> Peer {
        Peer {
            name: format!("Mac {id}"),
            id: id.to_string(),
            host: host.to_string(),
            token: token.to_string(),
            sunshine_paired: false,
            addresses: Vec::new(),
            placement: Default::default(),
            stream: Default::default(),
            allowed: true,
        }
    }

    // Final review I2: the end-of-session write-back inserted a forgotten Mac again and set it
    // current, and dropped Moonlight's pairing the session had just made.
    #[test]
    fn the_session_write_back_only_updates_a_mac_that_is_still_there() {
        let (a, _d) = app();
        a.replace_peer("a", mac_at("a", "192.168.1.20", &token('a'))).unwrap();
        a.replace_peer("b", mac_at("b", "192.168.1.21", &token('b'))).unwrap();
        a.make_current("a").unwrap();
        let at_start = a.cfg.lock().unwrap().peer("a").cloned().unwrap();
        // The user switches to b and re-pairs a while a's display is ending.
        a.make_current("b").unwrap();
        a.replace_peer("a", mac_at("a", "192.168.1.20", &token('c'))).unwrap();
        let after = Peer { sunshine_paired: true, addresses: vec!["10.0.0.5".into()], ..at_start.clone() };
        a.write_back_session("a", &after);
        let cfg = Config::load(&a.path).unwrap();
        assert_eq!(cfg.current.as_deref(), Some("b"), "the switch stands");
        let saved = cfg.peer("a").unwrap();
        assert!(saved.sunshine_paired, "what the session learnt is kept");
        assert_eq!(saved.addresses, vec!["10.0.0.5".to_string()]);
        assert_eq!(saved.token, token('c'), "the newer pairing stands");
        // Forgotten during the display: it stays forgotten.
        a.forget_mac("a").unwrap();
        a.write_back_session("a", &after);
        a.write_back_pairing("a", after.clone());
        assert!(Config::load(&a.path).unwrap().peer("a").is_none());
    }

    // Final review I6: the UI tells which PC holds the Mac, apart from `current`.
    #[test]
    fn the_status_marks_the_connected_peer() {
        let (a, _d) = app();
        a.pair_pc(a_pc("pc-1", &token('a'), true)).unwrap();
        a.pair_pc(a_pc("pc-2", &token('b'), true)).unwrap();
        assert!(a.status().peers.iter().all(|p| !p.connected && !p.current));
        a.set_mouse_status(MouseStatus::Connected { mac_id: "pc-2".into(), mac_name: "pc pc-2".into(), uses: None, problem: None, from: None, rtt_ms: None });
        let peers = a.status().peers;
        let connected: Vec<_> = peers.iter().filter(|p| p.connected).map(|p| p.key.as_str()).collect();
        assert_eq!(connected, vec!["pc-2"]);
        assert!(peers.iter().all(|p| !p.current), "connected is not current");
    }

    #[test]
    fn the_status_round_trips_the_viewer_monitor() {
        let (a, _d) = app();
        a.set_peer_host("192.168.1.20").unwrap();
        assert_eq!(a.status().settings.monitor, None);
        a.save_settings(StreamSettings { monitor: Some(2), ..settings(1920, 60, 20000) }).unwrap();
        assert_eq!(a.status().settings.monitor, Some(2));
    }

    // Final review I5: the Mac dialled the address a PC had when it paired.
    #[test]
    fn a_pc_seen_at_a_new_address_moves_there_and_keeps_the_old_one() {
        let mut pc = a_pc("pc-1", &token('a'), true);
        pc.host = "192.168.1.30".into();
        pc.addresses = vec!["100.64.0.2".into(), "192.168.1.31".into()];
        moved_to(&mut pc, "192.168.1.31");
        assert_eq!(pc.host, "192.168.1.31");
        assert_eq!(pc.addresses, vec!["192.168.1.30".to_string(), "100.64.0.2".to_string()]);
        moved_to(&mut pc, "192.168.1.31");
        assert_eq!(pc.addresses.len(), 2, "the same address changes nothing");
        for n in 0..20 {
            moved_to(&mut pc, &format!("10.0.0.{n}"));
        }
        assert_eq!(pc.addresses.len(), MAX_ADDRESSES);
        assert_eq!(pc.addresses[0], "10.0.0.18");
    }

    #[test]
    fn the_mac_records_where_the_connected_pc_is_and_writes_only_a_change() {
        let (a, _d) = app();
        a.pair_pc(a_pc("pc-1", &token('a'), true)).unwrap();
        let ip: std::net::IpAddr = "192.168.1.40".parse().unwrap();
        a.saw_pc_at("pc-1", ip);
        let saved = Config::load(&a.path).unwrap();
        assert_eq!(saved.peer("pc-1").unwrap().host, "192.168.1.40");
        assert_eq!(saved.peer("pc-1").unwrap().addresses, vec!["192.168.1.30".to_string()]);
        // Seen at the same address again: the file is not written.
        std::fs::remove_file(&a.path).unwrap();
        a.saw_pc_at("pc-1", ip);
        a.saw_pc_at("nobody", ip);
        assert!(!a.path.exists());
    }

    /// A Mac migrated from 1.0: its PC is known by token, not by id.
    fn migrated_mac() -> (AppState, tempfile::TempDir) {
        let (a, d) = app();
        a.pair_pc(a_pc("pc-1", &token('a'), true)).unwrap();
        let key = a.replace_peer("host:192.168.1.30", a_pc("", &token('b'), true)).unwrap();
        assert_eq!(key, "host:192.168.1.30");
        a.cfg.lock().unwrap().current = Some(key);
        (a, d)
    }

    // Live check, task 9: the PC proved itself as the migrated entry; the Mac records its id.
    #[test]
    fn the_mac_learns_the_id_of_the_pc_it_migrated() {
        let (a, _d) = migrated_mac();
        a.learn_pc("pc-real");
        let saved = Config::load(&a.path).unwrap();
        let pc = saved.peer("pc-real").expect("known by id now");
        assert_eq!((pc.token.as_str(), pc.name.as_str()), (token('b').as_str(), "pc "));
        assert!(saved.peer("host:192.168.1.30").is_none());
        assert_eq!(saved.current.as_deref(), Some("pc-real"), "current follows the entry");
        assert_eq!(saved.peers.len(), 2);
        // The next admission matches it by id.
        let pcs = known_pcs(&saved);
        assert!(pcs.iter().any(|p| p.0 == "pc-real"));
        assert_eq!(
            expand_core::share::admit::decide(&pcs, None, "pc-real"),
            expand_core::share::channel::Admit::Accept { token: token('b') }
        );
        // Learnt once: nothing more to learn, and a PC known by id never changes.
        std::fs::remove_file(&a.path).unwrap();
        a.learn_pc("pc-real");
        a.learn_pc("pc-1");
        a.learn_pc("pc-other");
        a.learn_pc("");
        assert!(!a.path.exists());
    }

    // Review of 838ca66, I2: the entry was read under one lock and written later, inserting.
    #[test]
    fn a_pc_forgotten_while_its_id_is_learnt_stays_forgotten() {
        let (a, _d) = migrated_mac();
        let seen = a.pc_to_learn("pc-real").unwrap();
        assert!(!a.forget_pc("host:192.168.1.30").unwrap());
        a.adopt_pc("pc-real", &seen);
        let cfg = a.cfg.lock().unwrap();
        assert!(cfg.peer("pc-real").is_none(), "the forgotten PC came back");
        assert_eq!(cfg.peers.len(), 1);
    }

    #[test]
    fn learning_the_same_pc_twice_at_once_keeps_what_changed_between() {
        let (a, _d) = migrated_mac();
        // The display door and the sharing channel both see the PC prove itself.
        let first = a.pc_to_learn("pc-real").unwrap();
        let second = a.pc_to_learn("pc-real").unwrap();
        a.adopt_pc("pc-real", &first);
        a.set_pc_allowed("pc-real", false).unwrap();
        a.adopt_pc("pc-real", &second);
        let saved = Config::load(&a.path).unwrap();
        assert!(!saved.peer("pc-real").unwrap().allowed, "the block was undone");
        assert_eq!(saved.peers.len(), 2);
    }

    // Review of 838ca66, minor: an entry without a token proves nothing, by id or not.
    #[test]
    fn a_pc_without_a_token_is_not_let_in_by_its_id() {
        let (a, _d) = app();
        a.cfg.lock().unwrap().peers.push(a_pc("pc-x", "", true));
        assert!(known_pc(&a.cfg.lock().unwrap(), "pc-x").is_none());
    }

    #[test]
    fn the_display_door_lets_the_migrated_pc_in_by_its_token() {
        let (a, _d) = migrated_mac();
        let cfg = a.cfg.lock().unwrap();
        let pc = known_pc(&cfg, "pc-real").expect("the id-less entry");
        assert_eq!((pc.id.as_str(), pc.token.as_str()), ("pc-real", token('b').as_str()));
        assert_eq!(known_pc(&cfg, "pc-1").unwrap().token, token('a'));
        assert!(known_pc(&cfg, "").is_none());
        drop(cfg);
        a.set_pc_allowed("host:192.168.1.30", false).unwrap();
        assert!(!known_pc(&a.cfg.lock().unwrap(), "pc-real").unwrap().allowed);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_mac_learns_the_pcs_id_when_it_connects() {
        let (a, _d) = migrated_mac();
        let ip: std::net::IpAddr = "192.168.1.41".parse().unwrap();
        a.set_mouse_status(MouseStatus::Connected { mac_id: "pc-real".into(), mac_name: "pc ".into(), uses: None, problem: None, from: Some(ip), rtt_ms: None });
        let saved = Config::load(&a.path).unwrap();
        assert_eq!(saved.peer("pc-real").unwrap().host, "192.168.1.41");
        assert_eq!(saved.current.as_deref(), Some("pc-real"));
        assert!(a.status().peers.iter().any(|p| p.connected && p.key == "pc-real"));
    }

    #[test]
    fn a_connected_pc_whose_input_cannot_be_replayed_is_still_connected() {
        let (a, _d) = app();
        a.set_mouse_status(MouseStatus::Connected {
            mac_id: "pc-1".into(),
            mac_name: "pc 1".into(),
            uses: None,
            problem: Some("Accessibility permission is missing".into()),
            from: None,
            rtt_ms: None,
        });
        let s = a.status();
        assert_eq!((s.connection, s.mouse_state), ("connected", "failed"));
        assert_eq!(s.mouse_reason.as_deref(), Some("Accessibility permission is missing"));
        assert_eq!(a.connected_pc().as_deref(), Some("pc-1"));
    }

    // Final review I6: pairing on the Mac does not move `current`.
    #[test]
    fn pairing_a_pc_does_not_change_the_current_entry() {
        let (a, _d) = app();
        a.pair_pc(a_pc("pc-1", &token('a'), true)).unwrap();
        assert_eq!(a.cfg.lock().unwrap().current, None);
    }

    /// A Mac's control-link door on 127.0.0.1 that pairs while `mac`'s pairing mode is open and
    /// stores each new PC, as the Mac app does.
    async fn mac_door(mac: std::sync::Arc<AppState>) -> u16 {
        let mut server = expand_core::link::server::Server::bind("127.0.0.1:0".parse().unwrap()).await.unwrap();
        let port = server.local_addr().port();
        tokio::spawn(async move {
            let gate: std::sync::Arc<dyn PairGate> = std::sync::Arc::new(PairingGate(mac.clone()));
            loop {
                let (name, id) = {
                    let cfg = mac.cfg.lock().unwrap();
                    (cfg.device_name.clone(), cfg.device_id.clone())
                };
                let for_lookup = mac.clone();
                let trusted = move |pc: &str| for_lookup.cfg.lock().unwrap().peer(pc).filter(|p| p.allowed).cloned();
                if let Ok((_, Some(pc), _)) = server.accept(trusted, gate.clone(), (&name, &id)).await {
                    let _ = mac.pair_pc(pc);
                }
            }
        });
        port
    }

    // WP1: wrong codes over the real link are refused, store nothing, and close pairing mode
    // after five; the right code is refused after that.
    #[tokio::test]
    async fn five_wrong_codes_over_the_link_close_pairing_and_store_nothing() {
        let (mac, _md) = shared();
        let port = mac_door(mac.clone()).await;
        let (pc, _pd) = shared();
        let code = mac.start_pairing();
        let wrong = if code == "000000" { "000001" } else { "000000" };
        let target = mac_at("m", "127.0.0.1", "");
        for _ in 0..PAIRING_ATTEMPTS {
            assert!(mac.pairing_open().is_some());
            let e = pc.pair_with_code(&target, wrong, port).await.map(|_| ()).unwrap_err();
            assert!(e.contains("declined"), "{e}");
        }
        assert_eq!(mac.pairing_open(), None, "five exchanges without the code close it");
        let e = pc.pair_with_code(&target, &code, port).await.map(|_| ()).unwrap_err();
        assert!(e.contains("declined"), "{e}");
        assert!(mac.cfg.lock().unwrap().peers.is_empty(), "the Mac stored nothing");
    }

    // WP1 review: the Mac runs handshakes side by side; wrong codes at the same time still
    // count exactly once each.
    #[tokio::test]
    async fn wrong_codes_at_the_same_time_each_count_once() {
        let (mac, _md) = shared();
        let port = mac_door(mac.clone()).await;
        let (pc, _pd) = shared();
        let code = mac.start_pairing();
        let wrong = if code == "000000" { "000001" } else { "000000" };
        let target = mac_at("m", "127.0.0.1", "");
        let try_one = || pc.pair_with_code(&target, wrong, port);
        let (a, b, c, d) = tokio::join!(try_one(), try_one(), try_one(), try_one());
        assert_eq!(PAIRING_ATTEMPTS, 5, "four at once is one short of the limit");
        for r in [a, b, c, d] {
            assert!(r.map(|_| ()).unwrap_err().contains("declined"));
        }
        assert!(mac.pairing_open().is_some(), "four misses, not five");
        assert!(pc.pair_with_code(&target, wrong, port).await.is_err());
        assert_eq!(mac.pairing_open(), None, "and the fifth closes it");
        assert!(pc.pair_with_code(&target, &code, port).await.is_err());
        assert!(mac.cfg.lock().unwrap().peers.is_empty());
    }

    async fn eventually(ok: impl Fn() -> bool) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while !ok() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("in time");
    }

    // Final review I1 + I2: pairing then connecting, end to end over the control link.
    #[tokio::test]
    async fn pairing_then_connecting_switches_only_after_it_worked() {
        let (mac, _md) = shared();
        mac.update(|cfg| cfg.device_name = "MacBook".into()).unwrap();
        let port = mac_door(mac.clone()).await;
        let (pc, _pd) = shared();
        // Connected to another Mac, with its display running.
        pc.replace_peer("a", mac_at("a", "192.168.1.20", &token('a'))).unwrap();
        pc.make_current("a").unwrap();
        let display = pc.begin_session().unwrap();
        let key = pc.add_peer_by_host("127.0.0.1").unwrap();

        // A wrong code: nothing switches, the display goes on.
        let code = mac.start_pairing();
        let wrong = if code == "000000" { "000001" } else { "000000" };
        assert!(pc.connect_to(Some(&key), Some(wrong), port).await.is_err());
        assert_eq!(pc.cfg.lock().unwrap().current.as_deref(), Some("a"));
        assert!(!display.is_cancelled());

        // No code and no token: asked for the code, nothing switches.
        pc.set_want_connection(false);
        assert_eq!(pc.connect_to(Some(&key), None, port).await, Err("Enter the Mac's code".to_string()));
        assert!(!pc.want_connection());
        assert_eq!(pc.cfg.lock().unwrap().current.as_deref(), Some("a"));

        // The right code: paired, renamed to the Mac's id, current, and the old display ends.
        pc.connect_to(Some(&key), Some(&format!(" {code} ")), port).await.unwrap();
        let mac_id = mac.cfg.lock().unwrap().device_id.clone();
        let first = {
            let cfg = pc.cfg.lock().unwrap();
            assert_eq!(cfg.current.as_deref(), Some(mac_id.as_str()));
            assert_eq!(cfg.peers.len(), 2, "the entry by address became the Mac's");
            let p = cfg.current_peer().unwrap().clone();
            assert_eq!((p.name.as_str(), p.host.as_str(), p.token.len()), ("MacBook", "127.0.0.1", 64));
            p.token
        };
        assert!(display.is_cancelled(), "the old Mac's display ends on the switch");
        assert!(pc.want_connection());
        let pc_id = pc.cfg.lock().unwrap().device_id.clone();
        eventually(|| mac.cfg.lock().unwrap().peer(&pc_id).map(|p| p.token.clone()) == Some(first.clone())).await;

        // The Mac forgets this PC: a typed code pairs again although a token is saved, and the
        // new token replaces the old one on both sides.
        mac.forget_pc(&pc_id).unwrap();
        let code = mac.start_pairing();
        pc.connect_to(None, Some(&code), port).await.unwrap();
        let second = pc.cfg.lock().unwrap().current_peer().unwrap().token.clone();
        assert_ne!(second, first);
        eventually(|| mac.cfg.lock().unwrap().peer(&pc_id).map(|p| p.token.clone()) == Some(second.clone())).await;

        // Choosing the paired Mac again without a code just connects to it.
        pc.make_current("a").unwrap();
        pc.connect_to(Some(&mac_id), None, port).await.unwrap();
        assert_eq!(pc.cfg.lock().unwrap().current.as_deref(), Some(mac_id.as_str()));
    }

    // Plan 2 final review, item 4: Add by address saved the Mac before pairing, so a wrong code
    // or a mistyped address left an entry that could not be removed.
    #[tokio::test]
    async fn adding_a_mac_by_address_saves_it_only_once_paired() {
        let (mac, _md) = shared();
        mac.update(|cfg| cfg.device_name = "MacBook".into()).unwrap();
        let port = mac_door(mac.clone()).await;
        let (pc, _pd) = shared();
        pc.replace_peer("a", mac_at("a", "192.168.1.20", &token('a'))).unwrap();
        pc.make_current("a").unwrap();
        let before = pc.cfg.lock().unwrap().clone();

        let code = mac.start_pairing();
        let wrong = if code == "000000" { "000001" } else { "000000" };
        assert!(pc.add_mac_by_address("127.0.0.1", wrong, port).await.is_err());
        assert!(pc.add_mac_by_address("not an address", &code, port).await.is_err());
        assert!(pc.add_mac_by_address("127.0.0.1", "", port).await.is_err(), "no code, no pairing");
        assert_eq!(pc.cfg.lock().unwrap().peers, before.peers);
        assert_eq!(pc.cfg.lock().unwrap().current, before.current);
        assert_eq!(Config::load(&pc.path).unwrap().peers, before.peers, "nothing saved either");

        pc.add_mac_by_address(" 127.0.0.1 ", &code, port).await.unwrap();
        let mac_id = mac.cfg.lock().unwrap().device_id.clone();
        let cfg = Config::load(&pc.path).unwrap();
        assert_eq!(cfg.peers.len(), 2);
        assert_eq!(cfg.current.as_deref(), Some(mac_id.as_str()));
        let added = cfg.peer(&mac_id).unwrap();
        assert_eq!((added.name.as_str(), added.host.as_str(), added.token.len()), ("MacBook", "127.0.0.1", 64));
        assert!(pc.want_connection());
    }

    #[test]
    fn a_known_id_is_never_overwritten() {
        let (a, _d) = app();
        paired(&a);
        a.learn_mac("someone-else", "Other");
        let cfg = Config::load(&a.path).unwrap();
        assert_eq!((cfg.peers.len(), cfg.current_peer().unwrap().id.as_str()), (1, "macid"));
        assert_eq!(cfg.current_peer().unwrap().name, "MacBook");
    }

    // Live check, task 9: the Mac called itself "Expand" until it could read its computer
    // name; the PC keeps the name of a Mac it knows in step with what the Mac says.
    #[test]
    fn a_known_mac_that_renamed_itself_is_shown_by_its_new_name() {
        let (a, _d) = app();
        paired(&a);
        a.learn_mac("macid", "Studio");
        let cfg = Config::load(&a.path).unwrap();
        let p = cfg.current_peer().unwrap();
        assert_eq!((p.id.as_str(), p.name.as_str(), p.token.len()), ("macid", "Studio", 64));
        assert!(p.sunshine_paired);
        // No name to go by: the one it has is kept.
        a.learn_mac("macid", "");
        assert_eq!(Config::load(&a.path).unwrap().current_peer().unwrap().name, "Studio");
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
        a.set_mouse_status(MouseStatus::Lost("port busy".into()));
        let s = a.status();
        assert_eq!(s.mouse_state, "failed");
        assert_eq!(s.mouse_reason.as_deref(), Some("port busy"));
        assert_eq!(s.mac_side, "right");
    }

    // ---- the Mac: pairing mode, several PCs ----

    use expand_core::link::PairGate;
    use std::time::{Duration, Instant};

    fn a_pc(id: &str, token: &str, allowed: bool) -> Peer {
        Peer {
            name: format!("pc {id}"),
            id: id.to_string(),
            host: "192.168.1.30".to_string(),
            token: token.to_string(),
            sunshine_paired: false,
            addresses: Vec::new(),
            placement: Default::default(),
            stream: Default::default(),
            allowed,
        }
    }

    fn token(c: char) -> String {
        c.to_string().repeat(64)
    }

    fn shared() -> (std::sync::Arc<AppState>, tempfile::TempDir) {
        let (a, d) = app();
        (std::sync::Arc::new(a), d)
    }

    #[test]
    fn pairing_mode_closes_after_two_minutes() {
        let (a, _d) = app();
        let t0 = Instant::now();
        assert_eq!(a.pairing_open_at(t0), None);
        let code = a.start_pairing_at(t0);
        assert_eq!(code.len(), 6);
        assert_eq!(a.pairing_open_at(t0), Some((code.clone(), 120)));
        assert_eq!(a.pairing_open_at(t0 + Duration::from_secs(90)), Some((code.clone(), 30)));
        assert_eq!(a.pairing_open_at(t0 + Duration::from_millis(119_500)), Some((code, 1)));
        assert_eq!(a.pairing_open_at(t0 + Duration::from_secs(120)), None);
        assert_eq!(a.pairing_open_at(t0 + Duration::from_secs(500)), None);
    }

    #[test]
    fn cancelling_pairing_mode_closes_it() {
        let (a, _d) = app();
        a.start_pairing();
        assert!(a.pairing_open().is_some());
        a.cancel_pairing();
        assert_eq!(a.pairing_open(), None);
    }

    #[test]
    fn pairing_closes_after_one_pc_pairs() {
        let (a, _d) = shared();
        let gate = PairingGate(a.clone());
        let code = a.start_pairing();
        assert_eq!(gate.code().as_deref(), Some(code.as_str()));
        assert!(gate.conclude(&code, true));
        assert_eq!(a.pairing_open(), None, "one PC per pairing session");
        // Another PC, a moment later, or one whose exchange was already under way.
        assert_eq!(gate.code(), None);
        assert!(!gate.conclude(&code, true));
    }

    #[test]
    fn the_gate_admits_nobody_while_pairing_mode_is_closed() {
        let (a, _d) = shared();
        let gate = PairingGate(a.clone());
        assert_eq!(gate.code(), None);
        // Even a PC that proved the code of a pairing mode that has ended since.
        let code = a.start_pairing();
        a.cancel_pairing();
        assert_eq!(gate.code(), None);
        assert!(!gate.conclude(&code, true));
    }

    #[test]
    fn a_few_wrong_codes_are_allowed_then_pairing_closes() {
        let (a, _d) = shared();
        let gate = PairingGate(a.clone());
        let code = a.start_pairing();
        for _ in 0..4 {
            assert!(!gate.conclude(&code, false));
            assert!(a.pairing_open().is_some());
        }
        assert!(!gate.conclude(&code, false));
        assert_eq!(a.pairing_open(), None, "the fifth exchange without the code closes it");
        assert_eq!(gate.code(), None);
        // An exchange that started before and proves the code now is too late.
        assert!(!gate.conclude(&code, true));
    }

    #[test]
    fn a_new_pairing_mode_starts_counting_wrong_codes_again() {
        let (a, _d) = shared();
        let gate = PairingGate(a.clone());
        let old = a.start_pairing();
        for _ in 0..4 {
            gate.conclude(&old, false);
        }
        let code = a.start_pairing();
        assert!(!gate.conclude(&code, false));
        assert!(a.pairing_open().is_some());
        assert!(gate.conclude(&code, true));
    }

    #[test]
    fn an_exchange_from_an_earlier_pairing_mode_counts_for_nothing_in_the_next() {
        let (a, _d) = shared();
        let gate = PairingGate(a.clone());
        let old = a.start_pairing();
        let mut code = a.start_pairing();
        while code == old {
            code = a.start_pairing();
        }
        assert!(!gate.conclude(&old, true), "the old code does not pair");
        for _ in 0..PAIRING_ATTEMPTS {
            gate.conclude(&old, false);
        }
        assert!(a.pairing_open().is_some(), "nor do its misses count");
        assert!(gate.conclude(&code, true));
    }

    #[test]
    fn an_expired_pairing_mode_is_closed_everywhere() {
        let (a, _d) = app();
        let t0 = Instant::now();
        let code = a.start_pairing_at(t0);
        assert!(a.pairing_ends_in_at(t0 + Duration::from_secs(60)).is_some());
        let late = t0 + PAIRING_WINDOW;
        assert_eq!(a.pairing_ends_in_at(late), None, "nothing left to wait for");
        assert_eq!(a.pairing_open_at(late), None);
        // The gate uses the same check: the right code after the window is refused.
        assert_eq!(a.pairing_code_at(late), None);
        assert!(!a.conclude_pairing_at(&code, true, late));
    }

    #[test]
    fn a_pc_that_pairs_while_pairing_mode_is_open_is_allowed_even_if_it_was_blocked() {
        let (a, _d) = app();
        a.pair_pc(a_pc("pc-1", &token('a'), true)).unwrap();
        a.set_pc_allowed("pc-1", false).unwrap();
        a.pair_pc(a_pc("pc-1", &token('b'), true)).unwrap();
        let cfg = a.cfg.lock().unwrap();
        assert_eq!(cfg.peers.len(), 1);
        let pc = cfg.peer("pc-1").unwrap();
        assert!(pc.allowed);
        assert_eq!(pc.token, token('b'));
    }

    #[test]
    fn re_pairing_keeps_where_the_pc_sits() {
        let (a, _d) = app();
        let mut first = a_pc("pc-1", &token('a'), true);
        first.placement.side = Side::Top;
        a.pair_pc(first).unwrap();
        a.pair_pc(a_pc("pc-1", &token('b'), true)).unwrap();
        assert_eq!(a.cfg.lock().unwrap().peer("pc-1").unwrap().placement.side, Side::Top);
    }

    #[test]
    fn several_pcs_can_be_paired_and_each_is_listed() {
        let (a, _d) = app();
        a.pair_pc(a_pc("pc-1", &token('a'), true)).unwrap();
        a.pair_pc(a_pc("pc-2", &token('b'), true)).unwrap();
        let status = a.status();
        assert_eq!(status.peers.len(), 2);
        assert!(status.peers.iter().all(|p| p.allowed && p.paired));
    }

    #[test]
    fn the_status_shows_the_code_while_pairing_mode_is_open() {
        let (a, _d) = app();
        assert!(a.status().pairing.is_none());
        let code = a.start_pairing();
        let shown = a.status().pairing.unwrap();
        assert_eq!(shown.code, code);
        assert!(shown.seconds_left > 115 && shown.seconds_left <= 120);
    }

    #[test]
    fn blocking_and_allowing_a_pc_is_saved() {
        let (a, _d) = app();
        a.pair_pc(a_pc("pc-1", &token('a'), true)).unwrap();
        a.set_pc_allowed("pc-1", false).unwrap();
        assert!(!Config::load(&a.path).unwrap().peer("pc-1").unwrap().allowed);
        a.set_pc_allowed("pc-1", true).unwrap();
        assert!(Config::load(&a.path).unwrap().peer("pc-1").unwrap().allowed);
        assert!(a.set_pc_allowed("nobody", true).is_err());
    }

    #[test]
    fn the_connected_pc_is_the_one_the_sharing_channel_reports() {
        let (a, _d) = app();
        assert_eq!(a.connected_pc(), None);
        a.set_mouse_status(MouseStatus::Connected { mac_id: "pc-1".into(), mac_name: "pc 1".into(), uses: None, problem: None, from: None, rtt_ms: None });
        assert_eq!(a.connected_pc().as_deref(), Some("pc-1"));
        a.set_mouse_status(MouseStatus::Connecting);
        assert_eq!(a.connected_pc(), None);
    }

    #[test]
    fn a_disconnect_during_a_restart_cuts_the_pc_when_it_is_back() {
        let (a, _d) = app();
        let t0 = Instant::now();
        a.note_restart("pc-1", t0);
        // In the gap nothing is connected; the Disconnect is kept for the restarting PC.
        assert_eq!(a.restarting_pc(t0 + Duration::from_secs(1)).as_deref(), Some("pc-1"));
        assert!(a.cut_when_back("pc-1", t0 + Duration::from_secs(1)));
        assert!(a.back_from_restart("pc-1", t0 + Duration::from_secs(3)), "cut on its return");
        assert!(!a.back_from_restart("pc-1", t0 + Duration::from_secs(4)), "once");
        assert_eq!(a.restarting_pc(t0 + Duration::from_secs(4)), None, "the restart is over");
    }

    #[test]
    fn a_restart_without_a_disconnect_ends_quietly_when_the_pc_is_back() {
        let (a, _d) = app();
        let t0 = Instant::now();
        a.note_restart("pc-1", t0);
        assert!(!a.cut_when_back("pc-2", t0), "another PC's cut is not kept");
        assert!(!a.back_from_restart("pc-1", t0 + Duration::from_secs(3)));
        assert!(!a.cut_when_back("pc-1", t0 + Duration::from_secs(4)), "the restart is over");
        assert!(!a.back_from_restart("pc-1", t0 + Duration::from_secs(5)));
    }

    #[test]
    fn a_restart_is_forgotten_after_its_window() {
        let (a, _d) = app();
        let t0 = Instant::now();
        a.note_restart("pc-1", t0);
        let late = t0 + RESTART_CUT_WINDOW + Duration::from_secs(1);
        assert_eq!(a.restarting_pc(late), None);
        assert!(!a.cut_when_back("pc-1", late));
        a.note_restart("pc-1", t0);
        assert!(a.cut_when_back("pc-1", t0));
        assert!(!a.back_from_restart("pc-1", late), "a late return is not cut");
    }

    #[test]
    fn stay_awake_only_while_a_pc_is_connected_and_the_setting_is_on() {
        let (a, _d) = app();
        assert!(!a.wants_awake());
        a.set_mouse_status(MouseStatus::Connected { mac_id: "pc-1".into(), mac_name: "pc 1".into(), uses: None, problem: None, from: None, rtt_ms: None });
        assert!(a.wants_awake(), "on by default");
        a.set_stay_awake(false).unwrap();
        assert!(!a.wants_awake());
        assert!(!a.stay_awake());
        a.set_stay_awake(true).unwrap();
        assert!(a.stay_awake());
        a.set_mouse_status(MouseStatus::Lost("gone".into()));
        assert!(!a.wants_awake());
    }

    #[test]
    fn forgetting_a_pc_removes_it_and_says_when_none_remain() {
        let (a, _d) = app();
        a.pair_pc(a_pc("pc-1", &token('a'), true)).unwrap();
        a.pair_pc(a_pc("pc-2", &token('b'), true)).unwrap();
        assert_eq!(a.forget_pc("pc-1"), Ok(false));
        assert_eq!(a.forget_pc("pc-2"), Ok(true));
        assert!(a.forget_pc("pc-2").is_err());
    }

    #[test]
    fn a_pcs_display_connection_can_be_cut() {
        let (a, _d) = app();
        let (first, second) = (a.register_control("pc-1"), a.register_control("pc-2"));
        a.kick_control("pc-1");
        assert!(first.0.is_cancelled());
        assert!(!second.0.is_cancelled());
        // A connection that has ended no longer stands in the way of the next one.
        a.unregister_control("pc-2", second.1);
        let again = a.register_control("pc-2");
        assert!(!again.0.is_cancelled());
    }

    #[test]
    fn a_pcs_side_is_learnt_for_that_pc_alone() {
        let (a, _d) = app();
        a.pair_pc(a_pc("pc-1", &token('a'), true)).unwrap();
        a.pair_pc(a_pc("pc-2", &token('b'), true)).unwrap();
        a.set_pc_side("pc-1", Side::Left).unwrap();
        let cfg = a.cfg.lock().unwrap();
        assert_eq!(cfg.peer("pc-1").unwrap().placement.side, Side::Left);
        assert_eq!(cfg.peer("pc-2").unwrap().placement.side, Side::Right);
    }

    // Final review I3: the display's restart raced the sharing connection and got "In use".
    #[test]
    fn a_pc_may_start_the_display_while_no_other_pc_is_connected() {
        let (a, _d) = app();
        assert!(a.may_start_display("pc-1"), "nobody is connected");
        assert!(!a.may_start_display("pc-2"), "the display is held");
        a.display_released("pc-1");
        a.set_mouse_status(MouseStatus::Connected { mac_id: "pc-2".into(), mac_name: "pc 2".into(), uses: None, problem: None, from: None, rtt_ms: None });
        assert!(!a.may_start_display("pc-1"), "another PC is connected");
    }

    // Plan 2 final review, item 2: the PC's Reconnect drops and re-opens its sharing link while
    // its display runs on; the Mac keeps the display for it meanwhile.
    #[test]
    fn the_display_stays_held_while_its_pcs_sharing_link_comes_back() {
        let (a, _d) = app();
        let connected = |id: &str| MouseStatus::Connected { mac_id: id.into(), mac_name: id.into(), uses: None, problem: None, from: None, rtt_ms: None };
        a.set_mouse_status(connected("pc-1"));
        assert!(a.may_start_display("pc-1"));
        a.set_mouse_status(MouseStatus::Off);
        assert!(!a.may_start_display("pc-2"), "the link is down, the display is still pc-1's");
        a.set_mouse_status(connected("pc-1"));
        assert!(!a.may_start_display("pc-2"));
        assert!(!a.may_start_display("pc-1"), "still held from before: not claimed twice");
        a.display_released("pc-1");
        assert!(a.may_start_display("pc-1"));
    }

    #[test]
    fn only_the_connected_pc_may_start_the_display_and_only_once() {
        let (a, _d) = app();
        a.set_mouse_status(MouseStatus::Connected { mac_id: "pc-1".into(), mac_name: "pc 1".into(), uses: None, problem: None, from: None, rtt_ms: None });
        assert!(!a.may_start_display("pc-2"));
        assert!(a.may_start_display("pc-1"));
        assert!(!a.may_start_display("pc-1"), "the display is already held");
        a.display_released("pc-2");
        assert!(!a.may_start_display("pc-1"), "only its holder gives it back");
        a.display_released("pc-1");
        assert!(a.may_start_display("pc-1"));
    }

    struct FakeEngines {
        fail: bool,
    }

    #[async_trait::async_trait]
    impl expand_core::session::HostEngines for FakeEngines {
        async fn start(&mut self, _w: u32, _h: u32, _fps: u32) -> Result<(), expand_core::sunshine::EngineError> {
            if self.fail { Err(expand_core::sunshine::EngineError::NotInstalled("Sunshine")) } else { Ok(()) }
        }
        async fn submit_pin(&mut self, _p: &str, _c: &str) -> Result<bool, expand_core::sunshine::EngineError> {
            Ok(true)
        }
        async fn stop(&mut self) {}
        async fn exited(&mut self) -> &'static str {
            "Sunshine"
        }
    }

    #[tokio::test]
    async fn the_running_display_size_is_noted_from_start_to_stop() {
        use expand_core::session::HostEngines;
        let (a, _d) = app();
        let mut e = SizeTracking { inner: FakeEngines { fail: false }, state: &a };
        assert_eq!(a.status().display, None);
        e.start(2560, 1440, 60).await.unwrap();
        assert_eq!(a.status().display, Some(DisplayDto { width: 2560, height: 1440, fps: 60 }));
        e.stop().await;
        assert_eq!(a.status().display, None);
        e.start(1920, 1080, 30).await.unwrap();
        e.exited().await;
        assert_eq!(a.status().display, None);
        let mut f = SizeTracking { inner: FakeEngines { fail: true }, state: &a };
        assert!(f.start(1920, 1080, 30).await.is_err());
        assert_eq!(a.status().display, None);
    }
}
