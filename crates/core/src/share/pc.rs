//! The PC's side of mouse sharing: when the pointer is pushed past the Mac-side edge, capture
//! the mouse and keyboard and send them to the Mac; when the Mac hands back, let go and put the
//! pointer where it belongs.

use super::channel::{pc_handshake, Refusal, ShareHandshakeError, INPUT_PORT};
use super::geometry::{along_edge, landing_on_pc, picture_area, Area, HandBack};
use super::keys::CmdKeys;
use super::wire::{latency_due, Input, Pace, ToMac, ToPc};
use crate::config::Placement;
use crate::mouse::{EndReason, Live, MouseStatus, Toggles, Uses};
use crate::side::Side;
use crate::winwindow;
use futures::StreamExt;
use input_capture::{Backend, CaptureEvent, InputCapture, Position};
use input_event::scancode::Linux;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

/// Ctrl+Shift+Alt+Win takes control back from the Mac, whatever state it is in.
const RELEASE_KEYS: [Linux; 4] = [Linux::KeyLeftCtrl, Linux::KeyLeftShift, Linux::KeyLeftMeta, Linux::KeyLeftAlt];
const RETRY: Duration = Duration::from_secs(3);

/// The connection's identity (the Mac's addresses, this PC's id, the token) decides whether it
/// must be restarted; what is shared, where the Mac sits and the stream's size never do.
#[derive(Debug, Clone)]
pub struct PcSettings {
    /// Every address the Mac may be reached at; whichever answers first is used.
    pub macs: Vec<IpAddr>,
    pub my_id: String,
    pub token: String,
    /// Where the Mac sits relative to the PC.
    pub placement: Placement,
    /// The stream's size, to find the picture inside the viewer window.
    pub stream: (u32, u32),
    /// What to share (mouse and keyboard, the clipboard, the keyboard language) and whether the
    /// display is in use. With `mouse` off nothing is shared; the connection stays open.
    pub toggles: Toggles,
}

/// Two settings are the same role when they reach the same Mac the same way; the rest is
/// pushed to the running connection instead (restarting released the mouse mid-use and raced
/// the display's restart, final review I3).
impl PartialEq for PcSettings {
    fn eq(&self, other: &PcSettings) -> bool {
        self.macs == other.macs && self.my_id == other.my_id && self.token == other.token
    }
}

impl PcSettings {
    /// What the running connection follows live.
    pub fn live(&self) -> Live {
        Live { toggles: self.toggles, placement: self.placement, stream: self.stream }
    }
}

impl Eq for PcSettings {}

/// How a session ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEnd {
    Network(String),
    /// The Mac closed after this PC's proof without proving itself: its token is not ours.
    Unproven,
    Ended(EndReason),
    Cancelled,
}

/// Only a lost network is worth another try (and a first unproven Mac, which may have been a
/// blip); a refusal, a goodbye or a stop is final.
pub fn retry_after(end: &SessionEnd) -> bool {
    matches!(end, SessionEnd::Network(_) | SessionEnd::Unproven)
}

/// How many unproven handshakes in a row mean the Mac no longer knows this PC's pairing.
const UNPROVEN_LIMIT: u32 = 2;

/// Turns the session's end into what is reported, counting unproven handshakes in a row: the
/// second one ends the run as `Unknown`, so the user is asked to pair again (final review I1).
fn judge(end: SessionEnd, unproven: &mut u32) -> SessionEnd {
    match end {
        SessionEnd::Unproven => {
            *unproven += 1;
            if *unproven >= UNPROVEN_LIMIT {
                SessionEnd::Ended(EndReason::Unknown)
            } else {
                SessionEnd::Unproven
            }
        }
        other => {
            *unproven = 0;
            other
        }
    }
}

fn position(side: Side) -> Position {
    match side {
        Side::Left => Position::Left,
        Side::Right => Position::Right,
        Side::Top => Position::Top,
        Side::Bottom => Position::Bottom,
    }
}

/// Runs until cancelled or ended (by the Mac, or a refusal), reconnecting only after the
/// network was lost. Sets `Ended` before returning when the run ended on its own.
pub async fn run(
    settings: PcSettings,
    live: watch::Receiver<Live>,
    status: watch::Sender<MouseStatus>,
    cancel: CancellationToken,
) {
    run_on(settings, live, status, cancel, INPUT_PORT).await
}

/// `run` against a chosen port (the Mac's is always `INPUT_PORT`; tests use another).
async fn run_on(
    settings: PcSettings,
    mut live: watch::Receiver<Live>,
    status: watch::Sender<MouseStatus>,
    cancel: CancellationToken,
    port: u16,
) {
    winwindow::use_physical_pixels();
    status.send_replace(MouseStatus::Connecting);
    let mut unproven = 0;
    loop {
        // The session watches `cancel` itself, so it can give control back properly first.
        let end = judge(session(&settings, &mut live, &status, &cancel, port).await, &mut unproven);
        if cancel.is_cancelled() {
            return;
        }
        match &end {
            SessionEnd::Network(reason) => {
                status.send_replace(MouseStatus::Lost(reason.clone()));
            }
            SessionEnd::Unproven => {
                status.send_replace(MouseStatus::Lost(ShareHandshakeError::Unproven.to_string()));
            }
            SessionEnd::Ended(reason) => {
                status.send_replace(MouseStatus::Ended(reason.clone()));
            }
            SessionEnd::Cancelled => {}
        }
        if !retry_after(&end) {
            return;
        }
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = tokio::time::sleep(RETRY) => {}
        }
    }
}

enum Next {
    Captured(Option<Result<(u64, CaptureEvent), input_capture::CaptureError>>),
    FromMac(Option<std::io::Result<ToPc>>),
    Heartbeat,
    ClipboardCheck,
    SendPiece,
    Changed(bool),
    Stop,
}

/// A send that cannot hang the loop: a Mac that stopped reading counts as gone, so the
/// capture (and the release shortcut) are never stuck behind a full connection.
async fn send_within<W: tokio::io::AsyncWrite + Unpin>(to_mac: &mut super::channel::Sender<W>, message: &ToMac) -> bool {
    matches!(tokio::time::timeout(Duration::from_secs(1), to_mac.send(message)).await, Ok(Ok(())))
}

const LOST: &str = "lost the connection to the Mac";

/// Starts watching the mouse and keyboard for the Mac's edge.
async fn open_capture(side: Side) -> Result<InputCapture, String> {
    let mut c = InputCapture::new(Some(Backend::Windows)).await.map_err(|e| format!("could not watch the mouse ({e})"))?;
    c.create(0, position(side)).await.map_err(|e| e.to_string())?;
    Ok(c)
}

fn new_unpacker(clipboard_on: bool) -> crate::clipboard::Unpacker {
    crate::clipboard::Unpacker::new(if clipboard_on { crate::clipboard::landing_folder() } else { std::env::temp_dir() })
}

async fn session(
    settings: &PcSettings,
    live: &mut watch::Receiver<Live>,
    status: &watch::Sender<MouseStatus>,
    cancel: &CancellationToken,
    port: u16,
) -> SessionEnd {
    let addrs: Vec<SocketAddr> = settings.macs.iter().map(|ip| SocketAddr::new(*ip, port)).collect();
    // A stop while the Mac is not answering is not kept waiting (it holds the app's lock).
    let reached = tokio::select! {
        _ = cancel.cancelled() => return SessionEnd::Cancelled,
        r = crate::link::client::first_reachable(&addrs, RETRY) => r,
    };
    let Some((stream, _)) = reached else {
        return SessionEnd::Network("the Mac is not answering; is Expand running there?".to_string());
    };
    let _ = stream.set_nodelay(true);
    let handshake = tokio::select! {
        _ = cancel.cancelled() => return SessionEnd::Cancelled,
        h = tokio::time::timeout(RETRY, pc_handshake(stream, &settings.my_id, &settings.token)) => h,
    };
    let (from_mac, mut to_mac, mac) = match handshake {
        Ok(Ok(((from_mac, to_mac), mac))) => (from_mac, to_mac, mac),
        Ok(Err(ShareHandshakeError::Refused(why))) => {
            return SessionEnd::Ended(match why {
                Refusal::Busy { by } => EndReason::Busy { by },
                Refusal::Blocked => EndReason::Blocked,
                Refusal::Unknown => EndReason::Unknown,
                Refusal::Version => EndReason::Version,
            })
        }
        Ok(Err(ShareHandshakeError::Unproven)) => return SessionEnd::Unproven,
        Ok(Err(ShareHandshakeError::Io(e))) => return SessionEnd::Network(format!("the Mac did not accept the connection ({e})")),
        Err(_) => return SessionEnd::Network("the Mac did not answer the mouse-sharing handshake".to_string()),
    };
    // With the mouse off nothing is shared; the connection just stays open.
    let first = *live.borrow_and_update();
    let (mut placement, mut stream_size) = (first.placement, first.stream);
    let now = first.toggles;
    let (mut mouse_on, mut clipboard_on, mut language_on) = (now.mouse, now.mouse && now.clipboard, now.mouse && now.language);
    let mut display_on = now.display;
    // Before anything is captured or reported as connected, so a failure here leaks nothing.
    let mut ctrl_as_cmd_on = now.ctrl_as_cmd;
    let uses = ToMac::Uses { mouse: mouse_on, clipboard: clipboard_on, display: display_on, ctrl_as_cmd: ctrl_as_cmd_on };
    if !send_within(&mut to_mac, &uses).await {
        return SessionEnd::Network(LOST.to_string());
    }
    let mut from_mac = from_mac.into_queue::<ToPc>();
    // Mouse and keyboard: only when switched on.
    let mut capture = if mouse_on {
        match open_capture(placement.side).await {
            Ok(c) => Some(c),
            Err(e) => return SessionEnd::Network(e),
        }
    } else {
        None
    };
    // Clipboard: only when switched on.
    let mut clipboard = if clipboard_on { crate::clipboard::Local::open() } else { None };
    let mut unpacker = new_unpacker(clipboard.is_some());
    let mut live_open = true;
    let mut watch = crate::clipboard::ChangeWatch::default();
    let mut seen_count = None;
    let mut clip_id = 0u64;
    let mut clip_tick = tokio::time::interval(CLIPBOARD_CHECK);
    // Large clipboard contents go out a piece at a time, between mouse and keyboard events.
    let mut outbox: std::collections::VecDeque<ToMac> = std::collections::VecDeque::new();
    let mut shortcuts = crate::language::Shortcuts::default();
    // Ctrl as Command on the Mac, Windows as Control (when switched on).
    let mut cmd_keys = CmdKeys::new(now.ctrl_as_cmd);
    // The wheel the other way round on the Mac (when switched on).
    let mut reverse_scroll = now.reverse_scroll;
    // The PC's keyboard layout while the Mac has the keyboard.
    let mut layout = 0usize;
    let mac_name = if mac.name.is_empty() { mac.id.clone() } else { mac.name.clone() };
    status.send_replace(MouseStatus::Connected { mac_id: mac.id, mac_name, uses: None, problem: None, from: None, rtt_ms: None });
    crate::sharelog::line(format!(
        "connected to the Mac (mouse {mouse_on}, clipboard {clipboard_on}, language {language_on})"
    ));

    // The monitor the pointer left from, while the Mac has control.
    let mut over_on_mac: Option<Area> = None;
    // Slow while nothing is in use; the Mac follows the same `Uses`.
    let mut pace = Pace::of(Some(Uses { mouse: mouse_on, clipboard: clipboard_on, display: display_on, ctrl_as_cmd: ctrl_as_cmd_on }));
    let mut heartbeat = tokio::time::interval(pace.heartbeat);
    // The Pings not answered yet, and the smoothed round trip so far.
    let mut pings = Pings::default();
    let mut rtt: Option<f64> = None;
    // The round trip last reported (to the status and the Mac), and when.
    let mut reported: Option<(u32, tokio::time::Instant)> = None;
    let mut last_heard = tokio::time::Instant::now();
    let result = loop {
        let next = tokio::select! {
            biased;
            _ = cancel.cancelled() => Next::Stop,
            e = next_captured(&mut capture) => Next::Captured(e),
            m = from_mac.recv() => Next::FromMac(m),
            _ = heartbeat.tick() => Next::Heartbeat,
            _ = clip_tick.tick(), if clipboard.is_some() => Next::ClipboardCheck,
            _ = std::future::ready(()), if !outbox.is_empty() => Next::SendPiece,
            changed = live.changed(), if live_open => Next::Changed(changed.is_ok()),
        };
        if matches!(next, Next::FromMac(Some(Ok(_)))) {
            last_heard = tokio::time::Instant::now();
        }
        match next {
            Next::Captured(Some(Ok((_, CaptureEvent::Begin)))) => {
                let at = winwindow::cursor().unwrap_or_default();
                let monitor = monitor_at(at);
                // A Mac under one monitor only takes the pointer from that monitor.
                // Known limit: Lan Mouse fires Bottom only at the global bottom edge, so a monitor
                // whose bottom is higher than another's cannot trigger this yet (follow-up).
                if !crate::layout::bottom_allows(&winwindow::monitors(), &monitor, &placement) {
                    if let Some(c) = capture.as_mut() {
                        if let Err(e) = c.release().await {
                            crate::sharelog::line(format!("releasing the mouse failed ({e})"));
                            break SessionEnd::Network(format!("releasing the mouse failed ({e})"));
                        }
                    }
                    continue;
                }
                let along = along_edge(monitor, at, placement.side);
                over_on_mac = Some(monitor);
                // Keys typed while Expand's main window is in front never reach the capture
                // (seen 2026-11-19); give the keyboard to the desktop while the Mac has control.
                // Not while the keyboard is on the embedded viewer (`should_step_aside`).
                winwindow::step_aside_if_in_front();
                shortcuts = crate::language::Shortcuts::default();
                cmd_keys.clear();
                if !send_within(&mut to_mac, &ToMac::Enter { pc_side: placement.side.opposite(), along }).await {
                    break SessionEnd::Network(LOST.to_string());
                }
                if language_on {
                    layout = winwindow::keyboard_layout();
                    if let Some(tag) = winwindow::language_tag(layout) {
                        let language = crate::language::primary_language(&tag);
                        if !send_within(&mut to_mac, &ToMac::Language(language)).await {
                            break SessionEnd::Network(LOST.to_string());
                        }
                    }
                }
            }
            Next::Captured(Some(Ok((_, CaptureEvent::Input(event))))) => {
                let Some(c) = capture.as_mut() else { continue };
                if c.keys_pressed(&RELEASE_KEYS) {
                    let _ = take_back(c, &mut to_mac, &mut cmd_keys).await;
                    over_on_mac = None;
                    continue;
                }
                // The PC's language shortcuts switch both machines' language.
                use crate::language::KeyAction;
                let action = match event {
                    input_event::Event::Keyboard(input_event::KeyboardEvent::Key { key, state, .. })
                        if language_on && over_on_mac.is_some() =>
                    {
                        shortcuts.key(key, state)
                    }
                    _ => KeyAction::Forward,
                };
                // The shortcuts above read the keys as pressed; the Mac gets them swapped.
                if matches!(action, KeyAction::Forward | KeyAction::ForwardAndSwitch)
                    && !send_all(&mut to_mac, cmd_keys.input(Input::from(event).scrolled(reverse_scroll))).await
                {
                    break SessionEnd::Network(LOST.to_string());
                }
                if matches!(action, KeyAction::ForwardAndSwitch | KeyAction::SwallowAndSwitch) {
                    let switched = winwindow::switch_keyboard_layout(layout);
                    if let Some(next) = switched {
                        layout = next;
                    }
                    let tag = switched.and_then(winwindow::language_tag);
                    crate::sharelog::line(format!("language shortcut: now {tag:?}"));
                    if let Some(tag) = tag {
                        let language = crate::language::primary_language(&tag);
                        if !send_within(&mut to_mac, &ToMac::Language(language)).await {
                            break SessionEnd::Network(LOST.to_string());
                        }
                    }
                }
            }
            Next::Captured(Some(Err(e))) => break SessionEnd::Network(format!("watching the mouse failed ({e})")),
            Next::Captured(None) => break SessionEnd::Network("watching the mouse stopped".to_string()),
            Next::FromMac(Some(Ok(ToPc::Pong))) => {
                if let Some(sent) = pings.answered() {
                    let smoothed = smooth(rtt, sent.elapsed().as_secs_f64() * 1000.0);
                    rtt = Some(smoothed);
                    let ms = rtt_ms(smoothed);
                    // Rarely and only when it moved, so neither window redraws every second.
                    if latency_due(reported.map(|(was, at)| (was, at.elapsed())), ms) {
                        reported = Some((ms, tokio::time::Instant::now()));
                        status.send_if_modified(|s| match s {
                            MouseStatus::Connected { rtt_ms, .. } if *rtt_ms != Some(ms) => {
                                *rtt_ms = Some(ms);
                                true
                            }
                            _ => false,
                        });
                        if !send_within(&mut to_mac, &ToMac::Latency { ms }).await {
                            break SessionEnd::Network(LOST.to_string());
                        }
                    }
                }
            }
            Next::FromMac(Some(Ok(ToPc::Disconnect))) => break SessionEnd::Ended(EndReason::ByMac),
            Next::FromMac(Some(Ok(ToPc::HandBack(back)))) => {
                if let Some(monitor) = over_on_mac.take() {
                    if let Some(c) = capture.as_mut() {
                        let _ = c.release().await;
                    }
                    winwindow::set_cursor(landing(&placement, stream_size, monitor, &back));
                }
            }
            Next::FromMac(Some(Ok(ToPc::Clip(message)))) => {
                if let (Some(local), Some(content)) = (clipboard.as_mut(), unpacker.feed(message)) {
                    if local.write(&content) {
                        watch.applied(&content);
                    }
                }
            }
            Next::FromMac(_) => break SessionEnd::Network(LOST.to_string()),
            Next::Heartbeat => {
                if last_heard.elapsed() > pace.silence {
                    break SessionEnd::Network("the Mac stopped answering".to_string());
                }
                // While idle nothing is timed at all.
                pings.sent((!pace.is_idle()).then(tokio::time::Instant::now));
                if !send_within(&mut to_mac, &ToMac::Ping).await {
                    break SessionEnd::Network(LOST.to_string());
                }
            }
            Next::ClipboardCheck => {
                let Some(local) = clipboard.as_mut() else { continue };
                let count = local.change_count();
                if seen_count == Some(count) {
                    continue;
                }
                seen_count = Some(count);
                let now = local.read();
                if watch.local_change(now.as_ref()) {
                    clip_id += 1;
                    if let Some(messages) = now.as_ref().and_then(|c| crate::clipboard::pack(c, clip_id)) {
                        outbox.extend(messages.into_iter().map(ToMac::Clip));
                    }
                }
            }
            Next::SendPiece => {
                if let Some(message) = outbox.pop_front() {
                    if !send_within(&mut to_mac, &message).await {
                        break SessionEnd::Network(LOST.to_string());
                    }
                }
            }
            Next::Changed(open) => {
                live_open = open;
                let next = *live.borrow_and_update();
                let now = next.toggles;
                let (mouse, clip, language) = (now.mouse, now.mouse && now.clipboard, now.mouse && now.language);
                // The capture watches one edge: the Mac moving to another side needs a new one.
                let moved = next.placement.side != placement.side;
                if !mouse || moved {
                    if let Some(mut c) = capture.take() {
                        // Give control back before the capture goes away.
                        if over_on_mac.take().is_some() && take_back(&mut c, &mut to_mac, &mut cmd_keys).await.is_err() {
                            break SessionEnd::Network(LOST.to_string());
                        }
                        let _ = c.release().await;
                        let _ = c.terminate().await;
                    }
                }
                (placement, stream_size) = (next.placement, next.stream);
                if mouse && capture.is_none() {
                    match open_capture(placement.side).await {
                        Ok(c) => capture = Some(c),
                        Err(e) => break SessionEnd::Network(e),
                    }
                }
                if clip && clipboard.is_none() {
                    clipboard = crate::clipboard::Local::open();
                    unpacker = new_unpacker(clipboard.is_some());
                    seen_count = None;
                }
                if !clip {
                    clipboard = None;
                    outbox.clear();
                }
                language_on = language;
                cmd_keys.set(now.ctrl_as_cmd);
                reverse_scroll = now.reverse_scroll;
                // Ctrl as ⌘ goes to the Mac too, for typing into the display window.
                if (mouse, clip, now.display, now.ctrl_as_cmd) != (mouse_on, clipboard_on, display_on, ctrl_as_cmd_on) {
                    let uses = ToMac::Uses { mouse, clipboard: clip, display: now.display, ctrl_as_cmd: now.ctrl_as_cmd };
                    if !send_within(&mut to_mac, &uses).await {
                        break SessionEnd::Network(LOST.to_string());
                    }
                }
                (mouse_on, clipboard_on, display_on, ctrl_as_cmd_on) = (mouse, clip, now.display, now.ctrl_as_cmd);
                let next_pace = Pace::of(Some(Uses { mouse, clipboard: clip, display: now.display, ctrl_as_cmd: now.ctrl_as_cmd }));
                if next_pace != pace {
                    pace = next_pace;
                    heartbeat = new_heartbeat(pace);
                    // The Mac was last heard up to an idle heartbeat ago: the faster pair's
                    // silence counts from now.
                    last_heard = tokio::time::Instant::now();
                    if pace.is_idle() {
                        // Nothing is measured while idle, and an old round trip is not shown
                        // as if current: the next busy stretch reports its first one at once.
                        pings.untime();
                        (rtt, reported) = (None, None);
                        status.send_if_modified(|s| match s {
                            MouseStatus::Connected { rtt_ms, .. } if rtt_ms.is_some() => {
                                *rtt_ms = None;
                                true
                            }
                            _ => false,
                        });
                    }
                }
            }
            Next::Stop => {
                if let (Some(c), Some(_)) = (capture.as_mut(), over_on_mac) {
                    let _ = take_back(c, &mut to_mac, &mut cmd_keys).await;
                }
                // Tell the Mac this is a goodbye, not a lost connection.
                let _ = send_within(&mut to_mac, &ToMac::Bye).await;
                break SessionEnd::Cancelled;
            }
        }
    };
    if let Some(mut c) = capture {
        let _ = c.release().await;
        let _ = c.terminate().await;
    }
    result
}

/// The heartbeat for a new pace: at once when something was turned on, so the link is
/// checked closely straight away; a whole slow interval on when everything went off.
fn new_heartbeat(pace: Pace) -> tokio::time::Interval {
    let now = tokio::time::Instant::now();
    let first = if pace.is_idle() { now + pace.heartbeat } else { now };
    tokio::time::interval_at(first, pace.heartbeat)
}

/// How often the clipboard's change counter is looked at.
const CLIPBOARD_CHECK: Duration = Duration::from_millis(400);

/// The next captured event; never ready without a capture.
async fn next_captured(
    capture: &mut Option<InputCapture>,
) -> Option<Result<(u64, CaptureEvent), input_capture::CaptureError>> {
    match capture {
        Some(c) => c.next().await,
        None => std::future::pending().await,
    }
}

/// Sends the Mac what it gets for one input from the PC (through Ctrl as Command: none, one or
/// two inputs). Whether all of it went.
async fn send_all<W: tokio::io::AsyncWrite + Unpin>(to_mac: &mut super::channel::Sender<W>, inputs: Vec<Input>) -> bool {
    for input in inputs {
        if !send_within(to_mac, &ToMac::Input(input)).await {
            return false;
        }
    }
    true
}

/// Takes control back from the Mac: lets go of the capture and tells the Mac, after letting go
/// of any key still held there (as the key the Mac got).
async fn take_back<W: tokio::io::AsyncWrite + Unpin>(
    capture: &mut InputCapture,
    to_mac: &mut super::channel::Sender<W>,
    cmd_keys: &mut CmdKeys,
) -> std::io::Result<()> {
    let held = capture.take_pressed_keys();
    let _ = capture.release().await;
    for key in held {
        // A key the Mac never got (a Ctrl still waiting to see whether it is AltGr's) is skipped.
        let Some(key) = cmd_keys.release(key as u32) else { continue };
        if !send_within(to_mac, &ToMac::Input(Input::Key { key, state: 0 })).await {
            return Err(std::io::Error::other(LOST));
        }
    }
    cmd_keys.clear();
    if send_within(to_mac, &ToMac::Leave).await {
        Ok(())
    } else {
        Err(std::io::Error::other(LOST))
    }
}

fn monitor_at(at: (f64, f64)) -> Area {
    let monitors = winwindow::monitors();
    monitors
        .iter()
        .copied()
        .find(|m| m.contains(at))
        .or_else(|| monitors.first().copied())
        .unwrap_or(Area { left: 0.0, top: 0.0, right: 1.0, bottom: 1.0 })
}

/// Where the pointer goes when the Mac hands back: onto the picture where the Mac's pointer was,
/// else (no picture showing: the display window minimized or hidden) along the edge.
fn landing(placement: &Placement, stream: (u32, u32), monitor: Area, back: &HandBack) -> (f64, f64) {
    let picture = winwindow::viewer_window()
        .and_then(winwindow::client_area)
        .map(|window| picture_area(window, stream));
    landing_on_pc(monitor, placement.side, back, picture)
}

/// The Pings not answered yet, oldest first, each with when it went out if it is timed. The Mac
/// answers every Ping with one Pong, in order, so each Pong is matched to its own Ping: the
/// answer to a Ping sent while idle is never timed against a newer one. Silence bounds how many
/// are outstanding (a few).
#[derive(Debug, Default)]
struct Pings(std::collections::VecDeque<Option<tokio::time::Instant>>);

impl Pings {
    /// A Ping went out; `timed` is when, if its answer is to be timed.
    fn sent(&mut self, timed: Option<tokio::time::Instant>) {
        self.0.push_back(timed);
    }

    /// A Pong came: when its own Ping went out, if that one was timed.
    fn answered(&mut self) -> Option<tokio::time::Instant> {
        self.0.pop_front().flatten()
    }

    /// Going idle: the answers still to come are not timed.
    fn untime(&mut self) {
        self.0.iter_mut().for_each(|p| *p = None);
    }
}

/// Weight of a new round-trip sample in the smoothed value.
const RTT_WEIGHT: f64 = 0.3;

/// The smoothed round trip after another sample (exponential moving average).
fn smooth(prev: Option<f64>, sample_ms: f64) -> f64 {
    match prev {
        Some(p) => p + RTT_WEIGHT * (sample_ms - p),
        None => sample_ms,
    }
}

fn rtt_ms(smoothed: f64) -> u32 {
    smoothed.round().max(0.0) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mouse::EndReason;

    #[test]
    fn two_unproven_handshakes_in_a_row_end_as_unknown() {
        let mut n = 0;
        assert_eq!(judge(SessionEnd::Unproven, &mut n), SessionEnd::Unproven);
        assert_eq!(judge(SessionEnd::Unproven, &mut n), SessionEnd::Ended(EndReason::Unknown));
        let mut n = 0;
        judge(SessionEnd::Unproven, &mut n);
        judge(SessionEnd::Network("blip".into()), &mut n);
        assert_eq!(judge(SessionEnd::Unproven, &mut n), SessionEnd::Unproven, "not in a row");
    }

    // Final review I1: a Mac that knows this PC but holds another token dropped it after its
    // proof; the PC called that a lost network and retried for ever.
    #[tokio::test]
    async fn a_mac_holding_another_token_ends_the_run_as_unknown() {
        tokio::task::LocalSet::new().run_until(async {
            let (l, port) = listener().await;
            let (status, mut rx) = watch::channel(MouseStatus::Off);
            let run = tokio::task::spawn_local(run_on(settings(), live(&settings()).1, status, CancellationToken::new(), port));
            let stale = |_: &str| Admit::Accept { token: "b".repeat(64) };
            let (stream, _) = l.accept().await.unwrap();
            assert!(mac_handshake(stream, &mac(), stale).await.is_err());
            wait_for(&mut rx, |s| matches!(s, MouseStatus::Lost(_))).await;
            let (stream, _) = tokio::time::timeout(Duration::from_secs(5), l.accept()).await.expect("a second try").unwrap();
            assert!(mac_handshake(stream, &mac(), stale).await.is_err());
            tokio::time::timeout(Duration::from_secs(5), run).await.expect("the run returned").unwrap();
            assert_eq!(*rx.borrow(), MouseStatus::Ended(EndReason::Unknown));
        }).await;
    }

    #[test]
    fn only_network_loss_is_retried() {
        assert!(retry_after(&SessionEnd::Network("gone".into())));
        for end in [
            SessionEnd::Cancelled,
            SessionEnd::Ended(EndReason::ByMac),
            SessionEnd::Ended(EndReason::Busy { by: "Other".into() }),
            SessionEnd::Ended(EndReason::Blocked),
            SessionEnd::Ended(EndReason::Unknown),
            SessionEnd::Ended(EndReason::Version),
        ] {
            assert!(!retry_after(&end), "{end:?}");
        }
    }

    use crate::share::channel::{mac_handshake, Admit, MacIdentity};
    use std::net::Ipv4Addr;
    use tokio::net::TcpListener;

    const TOKEN: &str = "token";

    fn settings() -> PcSettings {
        PcSettings {
            macs: vec![IpAddr::V4(Ipv4Addr::LOCALHOST)],
            my_id: "pc-1".into(),
            token: TOKEN.into(),
            placement: Placement::default(),
            stream: (1920, 1080),
            toggles: Toggles { mouse: false, clipboard: true, language: true, display: false, ctrl_as_cmd: true, reverse_scroll: false },
        }
    }

    /// The channel a manager would hold, starting at the settings' own live values.
    fn live(settings: &PcSettings) -> (watch::Sender<Live>, watch::Receiver<Live>) {
        watch::channel(settings.live())
    }

    async fn listener() -> (TcpListener, u16) {
        let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = l.local_addr().unwrap().port();
        (l, port)
    }

    fn mac() -> MacIdentity {
        MacIdentity { id: "mac-1".into(), name: "Studio".into() }
    }

    async fn wait_for(rx: &mut watch::Receiver<MouseStatus>, want: impl Fn(&MouseStatus) -> bool) {
        tokio::time::timeout(Duration::from_secs(5), rx.wait_for(|s| want(s))).await.expect("status in time").unwrap();
    }

    // Review: a Pong to an idle (untimed) Ping was read against the first busy Ping, reporting
    // a round trip of about 0 ms for 30 s.
    #[test]
    fn each_pong_answers_its_own_ping() {
        let t0 = tokio::time::Instant::now();
        let t1 = t0 + Duration::from_millis(5);
        let mut pings = Pings::default();
        pings.sent(None);
        pings.sent(Some(t1));
        assert_eq!(pings.answered(), None, "the idle Ping's answer is not timed");
        assert_eq!(pings.answered(), Some(t1), "the busy Ping's answer is");
        assert_eq!(pings.answered(), None, "an unasked-for Pong times nothing");
        pings.sent(Some(t0));
        pings.sent(Some(t1));
        pings.untime();
        pings.sent(Some(t1));
        assert_eq!((pings.answered(), pings.answered(), pings.answered()), (None, None, Some(t1)), "untimed when going idle");
    }

    #[test]
    fn round_trip_is_smoothed_and_rounded() {
        assert_eq!(smooth(None, 12.0), 12.0);
        assert!((smooth(Some(10.0), 20.0) - 13.0).abs() < 1e-9);
        assert_eq!(smooth(Some(10.0), 10.0), 10.0);
        assert_eq!(rtt_ms(12.6), 13);
    }

    #[tokio::test]
    async fn the_mac_ending_the_connection_ends_the_run_without_retrying() {
        tokio::task::LocalSet::new().run_until(async {
            let (l, port) = listener().await;
            let (status, mut rx) = watch::channel(MouseStatus::Off);
            let run = tokio::task::spawn_local(run_on(settings(), live(&settings()).1, status, CancellationToken::new(), port));
            let (stream, _) = l.accept().await.unwrap();
            let ((from_pc, mut to_pc), _) = mac_handshake(stream, &mac(), |_| Admit::Accept { token: TOKEN.into() }).await.unwrap();
            let mut from_pc = from_pc.into_queue::<ToMac>();
            // The first thing sent says nothing is shared, since the mouse is off.
            assert_eq!(from_pc.recv().await.unwrap().unwrap(), ToMac::Uses { mouse: false, clipboard: false, display: false, ctrl_as_cmd: true });
            wait_for(&mut rx, |s| matches!(s, MouseStatus::Connected { .. })).await;
            assert_eq!(
                *rx.borrow(),
                MouseStatus::Connected { mac_id: "mac-1".into(), mac_name: "Studio".into(), uses: None, problem: None, from: None, rtt_ms: None }
            );
            to_pc.send(&ToPc::Disconnect).await.unwrap();
            tokio::time::timeout(Duration::from_secs(5), run).await.expect("the run returned").unwrap();
            assert_eq!(*rx.borrow(), MouseStatus::Ended(EndReason::ByMac));
            // No second connection comes in.
            assert!(tokio::time::timeout(Duration::from_millis(200), l.accept()).await.is_err());
        }).await;
    }

    #[test]
    fn roles_with_different_toggles_placement_or_stream_are_the_same_connection() {
        let mut other = settings();
        other.toggles = Toggles { mouse: true, clipboard: false, language: false, display: true, ctrl_as_cmd: false, reverse_scroll: true };
        assert_eq!(settings(), other, "toggles never restart the connection");
        other.placement = Placement { side: crate::side::Side::Bottom, monitor: Some(2) };
        other.stream = (1280, 720);
        assert_eq!(settings(), other, "nor where the Mac sits or the stream's size");
        other.token = "another".into();
        assert_ne!(settings(), other);
        let mut moved = settings();
        moved.macs = vec![IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20))];
        assert_ne!(settings(), moved);
    }

    // Final review I3: a new placement or stream size reconnected and released the capture.
    #[tokio::test]
    async fn a_new_placement_or_stream_size_on_a_live_run_does_not_reconnect() {
        tokio::task::LocalSet::new().run_until(async {
            let (l, port) = listener().await;
            let (status, mut rx) = watch::channel(MouseStatus::Off);
            let cancel = CancellationToken::new();
            let (live_tx, live_rx) = live(&settings());
            let run = tokio::task::spawn_local(run_on(settings(), live_rx, status, cancel.clone(), port));
            let (stream, _) = l.accept().await.unwrap();
            let ((from_pc, _to_pc), _) = mac_handshake(stream, &mac(), |_| Admit::Accept { token: TOKEN.into() }).await.unwrap();
            let mut from_pc = from_pc.into_queue::<ToMac>();
            wait_for(&mut rx, |s| matches!(s, MouseStatus::Connected { .. })).await;
            live_tx.send_replace(Live {
                placement: Placement { side: crate::side::Side::Left, monitor: None },
                stream: (1280, 720),
                ..settings().live()
            });
            // Nothing but heartbeats and the first Uses: no goodbye, no second connection.
            let said = tokio::time::timeout(Duration::from_millis(400), async {
                loop {
                    match from_pc.recv().await {
                        Some(Ok(ToMac::Ping)) | Some(Ok(ToMac::Uses { .. })) => continue,
                        other => return other,
                    }
                }
            })
            .await;
            assert!(said.is_err(), "unexpected: {said:?}");
            assert!(tokio::time::timeout(Duration::from_millis(100), l.accept()).await.is_err());
            assert!(matches!(*rx.borrow(), MouseStatus::Connected { .. }));
            cancel.cancel();
            let _ = tokio::time::timeout(Duration::from_secs(5), run).await;
        }).await;
    }

    // Ledger T5: a stop while the Mac was not answering waited out the 3 s connect attempt.
    #[tokio::test]
    async fn stopping_while_the_mac_is_not_answering_returns_at_once() {
        tokio::task::LocalSet::new().run_until(async {
            let (status, _rx) = watch::channel(MouseStatus::Off);
            let cancel = CancellationToken::new();
            // A documentation-only address: nothing answers there.
            let mut nowhere = settings();
            nowhere.macs = vec![IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1))];
            let run = tokio::task::spawn_local(run_on(nowhere.clone(), live(&nowhere).1, status, cancel.clone(), 47801));
            tokio::time::sleep(Duration::from_millis(100)).await;
            let stopped = std::time::Instant::now();
            cancel.cancel();
            tokio::time::timeout(Duration::from_secs(5), run).await.expect("the run returned").unwrap();
            assert!(stopped.elapsed() < Duration::from_millis(500), "took {:?}", stopped.elapsed());
        }).await;
    }

    #[tokio::test]
    async fn changing_toggles_on_a_live_run_reports_them_without_reconnecting() {
        tokio::task::LocalSet::new().run_until(async {
            let (l, port) = listener().await;
            let (status, mut rx) = watch::channel(MouseStatus::Off);
            let cancel = CancellationToken::new();
            let (toggles_tx, toggles_rx) = live(&settings());
            let run = tokio::task::spawn_local(run_on(settings(), toggles_rx, status, cancel.clone(), port));
            let (stream, _) = l.accept().await.unwrap();
            let ((from_pc, _to_pc), _) = mac_handshake(stream, &mac(), |_| Admit::Accept { token: TOKEN.into() }).await.unwrap();
            let mut from_pc = from_pc.into_queue::<ToMac>();
            assert_eq!(from_pc.recv().await.unwrap().unwrap(), ToMac::Uses { mouse: false, clipboard: false, display: false, ctrl_as_cmd: true });
            wait_for(&mut rx, |s| matches!(s, MouseStatus::Connected { .. })).await;
            // The display starts: the Mac hears it on the same connection.
            toggles_tx.send_replace(Live { toggles: Toggles { display: true, ..settings().toggles }, ..settings().live() });
            let said = tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    match from_pc.recv().await.unwrap().unwrap() {
                        ToMac::Ping => continue,
                        m => return m,
                    }
                }
            })
            .await
            .expect("a new Uses in time");
            assert_eq!(said, ToMac::Uses { mouse: false, clipboard: false, display: true, ctrl_as_cmd: true });
            // And it ends again.
            toggles_tx.send_replace(settings().live());
            let said = tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    match from_pc.recv().await.unwrap().unwrap() {
                        ToMac::Ping => continue,
                        m => return m,
                    }
                }
            })
            .await
            .expect("a second Uses in time");
            assert_eq!(said, ToMac::Uses { mouse: false, clipboard: false, display: false, ctrl_as_cmd: true });
            // No goodbye, no second connection, still connected.
            assert!(tokio::time::timeout(Duration::from_millis(300), l.accept()).await.is_err());
            assert!(matches!(*rx.borrow(), MouseStatus::Connected { .. }));
            cancel.cancel();
            let _ = tokio::time::timeout(Duration::from_secs(5), run).await;
        }).await;
    }

    /// Ctrl as ⌘ alone changing is said to the Mac at once, on the same connection: the Mac
    /// applies it to typing into the display window.
    #[tokio::test]
    async fn turning_ctrl_as_cmd_over_tells_the_mac() {
        tokio::task::LocalSet::new().run_until(async {
            let (l, port) = listener().await;
            let (status, mut rx) = watch::channel(MouseStatus::Off);
            let cancel = CancellationToken::new();
            let (toggles_tx, toggles_rx) = live(&settings());
            let run = tokio::task::spawn_local(run_on(settings(), toggles_rx, status, cancel.clone(), port));
            let (stream, _) = l.accept().await.unwrap();
            let ((from_pc, _to_pc), _) = mac_handshake(stream, &mac(), |_| Admit::Accept { token: TOKEN.into() }).await.unwrap();
            let mut from_pc = from_pc.into_queue::<ToMac>();
            let first = ToMac::Uses { mouse: false, clipboard: false, display: false, ctrl_as_cmd: true };
            assert_eq!(from_pc.recv().await.unwrap().unwrap(), first);
            wait_for(&mut rx, |s| matches!(s, MouseStatus::Connected { .. })).await;
            toggles_tx.send_replace(Live { toggles: Toggles { ctrl_as_cmd: false, ..settings().toggles }, ..settings().live() });
            let said = tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    match from_pc.recv().await.unwrap().unwrap() {
                        ToMac::Ping => continue,
                        m => return m,
                    }
                }
            })
            .await
            .expect("a new Uses in time");
            assert_eq!(said, ToMac::Uses { mouse: false, clipboard: false, display: false, ctrl_as_cmd: false });
            assert!(tokio::time::timeout(Duration::from_millis(300), l.accept()).await.is_err(), "no reconnection");
            cancel.cancel();
            let _ = tokio::time::timeout(Duration::from_secs(5), run).await;
        }).await;
    }

    #[tokio::test]
    async fn stopping_says_goodbye_before_closing() {
        tokio::task::LocalSet::new().run_until(async {
            let (l, port) = listener().await;
            let (status, mut rx) = watch::channel(MouseStatus::Off);
            let cancel = CancellationToken::new();
            let run = tokio::task::spawn_local(run_on(settings(), live(&settings()).1, status, cancel.clone(), port));
            let (stream, _) = l.accept().await.unwrap();
            let ((from_pc, _to_pc), _) = mac_handshake(stream, &mac(), |_| Admit::Accept { token: TOKEN.into() }).await.unwrap();
            let mut from_pc = from_pc.into_queue::<ToMac>();
            wait_for(&mut rx, |s| matches!(s, MouseStatus::Connected { .. })).await;
            cancel.cancel();
            tokio::time::timeout(Duration::from_secs(5), run).await.expect("the run returned").unwrap();
            let mut got = Vec::new();
            while let Ok(Some(Ok(m))) = tokio::time::timeout(Duration::from_secs(2), from_pc.recv()).await {
                let bye = m == ToMac::Bye;
                got.push(m);
                if bye {
                    break;
                }
            }
            assert_eq!(got.last(), Some(&ToMac::Bye), "{got:?}");
        }).await;
    }

    // Plan 4, task 2: the Mac's Restart drops the link without ToPc::Disconnect; the PC takes it
    // for a lost network and connects again.
    #[tokio::test]
    async fn a_mac_that_drops_the_link_without_a_word_is_reconnected() {
        tokio::task::LocalSet::new().run_until(async {
            let (l, port) = listener().await;
            let (status, mut rx) = watch::channel(MouseStatus::Off);
            let cancel = CancellationToken::new();
            let run = tokio::task::spawn_local(run_on(settings(), live(&settings()).1, status, cancel.clone(), port));
            let (stream, _) = l.accept().await.unwrap();
            let ((from_pc, to_pc), _) = mac_handshake(stream, &mac(), |_| Admit::Accept { token: TOKEN.into() }).await.unwrap();
            wait_for(&mut rx, |s| matches!(s, MouseStatus::Connected { .. })).await;
            drop((from_pc, to_pc));
            wait_for(&mut rx, |s| matches!(s, MouseStatus::Lost(_))).await;
            let (stream, _) = tokio::time::timeout(Duration::from_secs(8), l.accept()).await.expect("a second try").unwrap();
            mac_handshake(stream, &mac(), |_| Admit::Accept { token: TOKEN.into() }).await.unwrap();
            wait_for(&mut rx, |s| matches!(s, MouseStatus::Connected { .. })).await;
            cancel.cancel();
            let _ = tokio::time::timeout(Duration::from_secs(5), run).await;
        }).await;
    }

    /// Settings with the display in use, so the link keeps the fast pace (the mouse would open
    /// a real capture).
    fn busy() -> PcSettings {
        PcSettings { toggles: Toggles { display: true, ..settings().toggles }, ..settings() }
    }

    // Plan 4, task 2: the Mac shows the PC's latency, as the PC measures it. Owner 2026-11-23:
    // only the first measurement, then at most every 30 s and when it moved (`latency_due`).
    #[tokio::test]
    async fn the_pc_reports_its_first_round_trip_and_then_keeps_it_steady() {
        tokio::task::LocalSet::new().run_until(async {
            let (l, port) = listener().await;
            let (status, mut rx) = watch::channel(MouseStatus::Off);
            let cancel = CancellationToken::new();
            let (live_tx, live_rx) = live(&busy());
            let run = tokio::task::spawn_local(run_on(busy(), live_rx, status, cancel.clone(), port));
            let (stream, _) = l.accept().await.unwrap();
            let ((from_pc, mut to_pc), _) = mac_handshake(stream, &mac(), |_| Admit::Accept { token: TOKEN.into() }).await.unwrap();
            let mut from_pc = from_pc.into_queue::<ToMac>();
            let reported = tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    match from_pc.recv().await.unwrap().unwrap() {
                        ToMac::Ping => to_pc.send(&ToPc::Pong).await.unwrap(),
                        ToMac::Latency { ms } => return ms,
                        _ => {}
                    }
                }
            })
            .await
            .expect("a Latency in time");
            wait_for(&mut rx, |s| matches!(s, MouseStatus::Connected { rtt_ms: Some(_), .. })).await;
            assert!(matches!(*rx.borrow_and_update(), MouseStatus::Connected { rtt_ms: Some(ms), .. } if ms == reported));
            // Pings go on every second, answered, but nothing more is reported for now.
            let quiet = tokio::time::timeout(Duration::from_millis(2500), async {
                loop {
                    match from_pc.recv().await.unwrap().unwrap() {
                        ToMac::Ping => to_pc.send(&ToPc::Pong).await.unwrap(),
                        ToMac::Latency { ms } => panic!("a second Latency ({ms} ms) too soon"),
                        _ => {}
                    }
                }
            })
            .await;
            assert!(quiet.is_err());
            assert!(!rx.has_changed().unwrap(), "the status was not announced again");
            // Review: everything off, and an old round trip is not shown as if current.
            live_tx.send_replace(settings().live());
            wait_for(&mut rx, |s| matches!(s, MouseStatus::Connected { rtt_ms: None, .. })).await;
            cancel.cancel();
            let _ = tokio::time::timeout(Duration::from_secs(5), run).await;
        }).await;
    }

    // Owner 2026-11-23: no traffic to speak of while nothing is in use. An idle link pings every
    // 15 s, never times the answer, and its 15 s heartbeat finds the Mac quiet for 15 s without
    // giving up (`IDLE_SILENCE`, not the fast pair's 3 s).
    #[tokio::test]
    async fn an_idle_link_is_not_measured_and_outlasts_the_fast_silence() {
        tokio::task::LocalSet::new().run_until(async {
            let (l, port) = listener().await;
            let (status, mut rx) = watch::channel(MouseStatus::Off);
            let cancel = CancellationToken::new();
            let run = tokio::task::spawn_local(run_on(settings(), live(&settings()).1, status, cancel.clone(), port));
            let (stream, _) = l.accept().await.unwrap();
            let ((from_pc, mut to_pc), _) = mac_handshake(stream, &mac(), |_| Admit::Accept { token: TOKEN.into() }).await.unwrap();
            let mut from_pc = from_pc.into_queue::<ToMac>();
            assert_eq!(from_pc.recv().await.unwrap().unwrap(), ToMac::Uses { mouse: false, clipboard: false, display: false, ctrl_as_cmd: true });
            wait_for(&mut rx, |s| matches!(s, MouseStatus::Connected { .. })).await;
            // The first heartbeat goes out at once; its answer is not timed.
            assert_eq!(from_pc.recv().await.unwrap().unwrap(), ToMac::Ping);
            to_pc.send(&ToPc::Pong).await.unwrap();
            tokio::time::sleep(Duration::from_millis(300)).await;
            assert!(from_pc.try_recv().is_err(), "no Latency while idle");
            assert!(matches!(*rx.borrow(), MouseStatus::Connected { rtt_ms: None, .. }));
            // Then 10 s go by without a word either way...
            tokio::time::pause();
            tokio::time::sleep(Duration::from_secs(10)).await;
            assert!(from_pc.try_recv().is_err(), "no Ping within 10 s while idle");
            // ...and 10 more, past the next heartbeat, which checks the silence and pings.
            tokio::time::sleep(Duration::from_secs(10)).await;
            tokio::time::resume();
            assert!(matches!(*rx.borrow(), MouseStatus::Connected { .. }), "still connected: {:?}", *rx.borrow());
            let ping = tokio::time::timeout(Duration::from_secs(1), from_pc.recv()).await.expect("the 15 s Ping");
            assert_eq!(ping.unwrap().unwrap(), ToMac::Ping);
            cancel.cancel();
            let _ = tokio::time::timeout(Duration::from_secs(5), run).await;
        }).await;
    }

    // Turning something on after a long idle stretch must not count the idle quiet against the
    // fast pair's 3 s of silence, and from then on the fast pair applies.
    #[tokio::test]
    async fn turning_something_on_after_idling_goes_back_to_the_fast_pair() {
        tokio::task::LocalSet::new().run_until(async {
            let (l, port) = listener().await;
            let (status, mut rx) = watch::channel(MouseStatus::Off);
            let cancel = CancellationToken::new();
            let (live_tx, live_rx) = live(&settings());
            let run = tokio::task::spawn_local(run_on(settings(), live_rx, status, cancel.clone(), port));
            let (stream, _) = l.accept().await.unwrap();
            let ((from_pc, mut to_pc), _) = mac_handshake(stream, &mac(), |_| Admit::Accept { token: TOKEN.into() }).await.unwrap();
            let mut from_pc = from_pc.into_queue::<ToMac>();
            wait_for(&mut rx, |s| matches!(s, MouseStatus::Connected { .. })).await;
            to_pc.send(&ToPc::Pong).await.unwrap();
            tokio::time::pause();
            tokio::time::sleep(Duration::from_secs(10)).await;
            tokio::time::resume();
            live_tx.send_replace(busy().live());
            // A Ping goes out at once and the link stands, although nothing was heard for 10 s.
            let pinged = tokio::time::timeout(Duration::from_millis(800), async {
                let mut uses = false;
                loop {
                    match from_pc.recv().await.unwrap().unwrap() {
                        ToMac::Uses { display: true, .. } => uses = true,
                        ToMac::Ping if uses => return,
                        _ => {}
                    }
                }
            })
            .await;
            assert!(pinged.is_ok(), "the new Uses, then a Ping at once");
            assert!(matches!(*rx.borrow(), MouseStatus::Connected { .. }), "still connected: {:?}", *rx.borrow());
            // Left unanswered, the fast pair gives up within a few seconds.
            let quiet = std::time::Instant::now();
            tokio::time::timeout(Duration::from_secs(6), rx.wait_for(|s| matches!(s, MouseStatus::Lost(_))))
                .await
                .expect("lost on the fast pair")
                .unwrap();
            assert!(quiet.elapsed() >= Duration::from_secs(2), "lost too soon: {:?}", quiet.elapsed());
            cancel.cancel();
            let _ = tokio::time::timeout(Duration::from_secs(5), run).await;
        }).await;
    }

    #[tokio::test]
    async fn a_busy_mac_ends_the_run_without_retrying() {
        tokio::task::LocalSet::new().run_until(async {
            let (l, port) = listener().await;
            let (status, rx) = watch::channel(MouseStatus::Off);
            let run = tokio::task::spawn_local(run_on(settings(), live(&settings()).1, status, CancellationToken::new(), port));
            let (stream, _) = l.accept().await.unwrap();
            let _ = mac_handshake(stream, &mac(), |_| Admit::Refuse(Refusal::Busy { by: "Other PC".into() })).await;
            tokio::time::timeout(Duration::from_secs(5), run).await.expect("the run returned").unwrap();
            assert_eq!(*rx.borrow(), MouseStatus::Ended(EndReason::Busy { by: "Other PC".into() }));
            assert!(tokio::time::timeout(Duration::from_millis(200), l.accept()).await.is_err());
        }).await;
    }
}
