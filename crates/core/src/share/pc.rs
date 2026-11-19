//! The PC's side of mouse sharing: when the pointer is pushed past the Mac-side edge, capture
//! the mouse and keyboard and send them to the Mac; when the Mac hands back, let go and put the
//! pointer where it belongs.

use super::channel::{pc_handshake, INPUT_PORT};
use super::geometry::{along_edge, landing_on_pc, picture_area, Area, HandBack};
use super::wire::{Input, ToMac, ToPc};
use crate::side::Side;
use crate::mouse::MouseStatus;
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PcSettings {
    /// Every address the Mac may be reached at; whichever answers first is used.
    pub macs: Vec<IpAddr>,
    pub my_id: String,
    pub token: String,
    /// Which side of the PC the Mac sits on.
    pub mac_side: Side,
    /// The stream's size, to find the picture inside the viewer window.
    pub stream: (u32, u32),
    /// What to share: mouse and keyboard, the clipboard, the keyboard language.
    pub mouse: bool,
    pub clipboard: bool,
    pub language: bool,
}

fn position(side: Side) -> Position {
    match side {
        Side::Left => Position::Left,
        Side::Right => Position::Right,
        Side::Top => Position::Top,
        Side::Bottom => Position::Bottom,
    }
}

/// Runs until cancelled, reconnecting to the Mac whenever the connection drops.
pub async fn run(settings: PcSettings, status: watch::Sender<MouseStatus>, cancel: CancellationToken) {
    winwindow::use_physical_pixels();
    loop {
        // The session watches `cancel` itself, so it can give control back properly first.
        let outcome = session(&settings, &status, &cancel).await;
        if cancel.is_cancelled() {
            return;
        }
        if let Err(reason) = outcome {
            status.send_replace(MouseStatus::Failed(reason));
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
    Stop,
}

/// A send that cannot hang the loop: a Mac that stopped reading counts as gone, so the
/// capture (and the release shortcut) are never stuck behind a full connection.
async fn send_within<W: tokio::io::AsyncWrite + Unpin>(to_mac: &mut super::channel::Sender<W>, message: &ToMac) -> bool {
    matches!(tokio::time::timeout(Duration::from_secs(1), to_mac.send(message)).await, Ok(Ok(())))
}

const LOST: &str = "lost the connection to the Mac";

async fn session(settings: &PcSettings, status: &watch::Sender<MouseStatus>, cancel: &CancellationToken) -> Result<(), String> {
    let addrs: Vec<SocketAddr> = settings.macs.iter().map(|ip| SocketAddr::new(*ip, INPUT_PORT)).collect();
    let Some((stream, _)) = crate::link::client::first_reachable(&addrs, RETRY).await else {
        return Err("the Mac is not answering; is Sidescreen running there?".to_string());
    };
    let _ = stream.set_nodelay(true);
    let (from_mac, mut to_mac) = match tokio::time::timeout(RETRY, pc_handshake(stream, &settings.my_id, &settings.token)).await {
        Ok(Ok(halves)) => halves,
        Ok(Err(e)) => return Err(format!("the Mac refused mouse sharing ({e})")),
        Err(_) => return Err("the Mac did not answer the mouse-sharing handshake".to_string()),
    };
    let mut from_mac = from_mac.into_queue::<ToPc>();
    // Mouse and keyboard: only when switched on.
    let mut capture = if settings.mouse {
        let mut c = InputCapture::new(Some(Backend::Windows))
            .await
            .map_err(|e| format!("could not watch the mouse ({e})"))?;
        c.create(0, position(settings.mac_side)).await.map_err(|e| e.to_string())?;
        Some(c)
    } else {
        None
    };
    // Clipboard: only when switched on.
    let mut clipboard = if settings.clipboard { crate::clipboard::Local::open() } else { None };
    let mut unpacker = crate::clipboard::Unpacker::new(if clipboard.is_some() {
        crate::clipboard::landing_folder()
    } else {
        std::env::temp_dir()
    });
    let mut watch = crate::clipboard::ChangeWatch::default();
    let mut seen_count = None;
    let mut clip_id = 0u64;
    let mut clip_tick = tokio::time::interval(CLIPBOARD_CHECK);
    // Large clipboard contents go out a piece at a time, between mouse and keyboard events.
    let mut outbox: std::collections::VecDeque<ToMac> = std::collections::VecDeque::new();
    let mut shortcuts = crate::language::Shortcuts::default();
    // The PC's keyboard layout while the Mac has the keyboard.
    let mut layout = 0usize;
    status.send_replace(MouseStatus::Running);
    crate::sharelog::line(format!(
        "connected to the Mac (mouse {}, clipboard {}, language {})",
        settings.mouse, settings.clipboard, settings.language
    ));

    // The monitor the pointer left from, while the Mac has control.
    let mut over_on_mac: Option<Area> = None;
    let mut heartbeat = tokio::time::interval(super::wire::HEARTBEAT);
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
        };
        if matches!(next, Next::FromMac(Some(Ok(_)))) {
            last_heard = tokio::time::Instant::now();
        }
        match next {
            Next::Captured(Some(Ok((_, CaptureEvent::Begin)))) => {
                let at = winwindow::cursor().unwrap_or_default();
                let monitor = monitor_at(at);
                let along = along_edge(monitor, at, settings.mac_side);
                over_on_mac = Some(monitor);
                // Keys typed while Sidescreen's own window is in front never reach the capture
                // (seen 2026-11-19); give the keyboard to the desktop while the Mac has control.
                winwindow::step_aside_if_in_front();
                shortcuts = crate::language::Shortcuts::default();
                if !send_within(&mut to_mac, &ToMac::Enter { pc_side: settings.mac_side.opposite(), along }).await {
                    break Err(LOST.to_string());
                }
                if settings.language {
                    layout = winwindow::keyboard_layout();
                    if let Some(tag) = winwindow::language_tag(layout) {
                        let language = crate::language::primary_language(&tag);
                        if !send_within(&mut to_mac, &ToMac::Language(language)).await {
                            break Err(LOST.to_string());
                        }
                    }
                }
            }
            Next::Captured(Some(Ok((_, CaptureEvent::Input(event))))) => {
                let Some(c) = capture.as_mut() else { continue };
                if c.keys_pressed(&RELEASE_KEYS) {
                    let _ = take_back(c, &mut to_mac).await;
                    over_on_mac = None;
                    continue;
                }
                // The PC's language shortcuts switch both machines' language.
                use crate::language::KeyAction;
                let action = match event {
                    input_event::Event::Keyboard(input_event::KeyboardEvent::Key { key, state, .. })
                        if settings.language && over_on_mac.is_some() =>
                    {
                        shortcuts.key(key, state)
                    }
                    _ => KeyAction::Forward,
                };
                if matches!(action, KeyAction::Forward | KeyAction::ForwardAndSwitch)
                    && !send_within(&mut to_mac, &ToMac::Input(Input::from(event))).await
                {
                    break Err(LOST.to_string());
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
                            break Err(LOST.to_string());
                        }
                    }
                }
            }
            Next::Captured(Some(Err(e))) => break Err(format!("watching the mouse failed ({e})")),
            Next::Captured(None) => break Err("watching the mouse stopped".to_string()),
            Next::FromMac(Some(Ok(ToPc::Pong))) => {}
            Next::FromMac(Some(Ok(ToPc::HandBack(back)))) => {
                if let Some(monitor) = over_on_mac.take() {
                    if let Some(c) = capture.as_mut() {
                        let _ = c.release().await;
                    }
                    winwindow::set_cursor(landing(settings, monitor, &back));
                }
            }
            Next::FromMac(Some(Ok(ToPc::Clip(message)))) => {
                if let (Some(local), Some(content)) = (clipboard.as_mut(), unpacker.feed(message)) {
                    if local.write(&content) {
                        watch.applied(&content);
                    }
                }
            }
            Next::FromMac(_) => break Err(LOST.to_string()),
            Next::Heartbeat => {
                if last_heard.elapsed() > super::wire::SILENCE {
                    break Err("the Mac stopped answering".to_string());
                }
                if !send_within(&mut to_mac, &ToMac::Ping).await {
                    break Err(LOST.to_string());
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
                        break Err(LOST.to_string());
                    }
                }
            }
            Next::Stop => {
                if let (Some(c), Some(_)) = (capture.as_mut(), over_on_mac) {
                    let _ = take_back(c, &mut to_mac).await;
                }
                break Ok(());
            }
        }
    };
    if let Some(mut c) = capture {
        let _ = c.release().await;
        let _ = c.terminate().await;
    }
    result
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

/// Takes control back from the Mac: lets go of the capture and tells the Mac, after letting go
/// of any key still held there.
async fn take_back<W: tokio::io::AsyncWrite + Unpin>(
    capture: &mut InputCapture,
    to_mac: &mut super::channel::Sender<W>,
) -> std::io::Result<()> {
    let held = capture.take_pressed_keys();
    let _ = capture.release().await;
    for key in held {
        if !send_within(to_mac, &ToMac::Input(Input::Key { key: key as u32, state: 0 })).await {
            return Err(std::io::Error::other(LOST));
        }
    }
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

/// Where the pointer goes when the Mac hands back.
fn landing(settings: &PcSettings, monitor: Area, back: &HandBack) -> (f64, f64) {
    let picture = winwindow::find_window_of_process(winwindow::VIEWER_EXE)
        .and_then(winwindow::client_area)
        .map(|window| picture_area(window, settings.stream));
    landing_on_pc(monitor, settings.mac_side, back, picture)
}
