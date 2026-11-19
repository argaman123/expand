//! The Mac's side of mouse sharing: listens for the trusted PC and follows its mouse and
//! keyboard. The decisions live in `follower`; this file only talks to macOS.

use super::channel::{mac_handshake, INPUT_PORT};
use super::follower::{Follower, MacDesk, Replay};
use super::geometry::Area;
use super::wire::{Input, ToMac, ToPc};
use crate::mouse::MouseStatus;
use async_trait::async_trait;
use core_graphics::display::{CGDisplay, CGPoint, CGRect};
use core_graphics::event::{CGEvent, CGEventTapLocation, CGEventType, CGMouseButton, EventField};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use input_emulation::{Backend, InputEmulation};
use std::collections::HashSet;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacSettings {
    /// The trusted PC's id and pairing token.
    pub pc_id: String,
    pub token: String,
    /// Share the clipboard as well as following the mouse and keyboard.
    pub clipboard: bool,
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventSourceButtonState(state_id: i32, button: u32) -> bool;
}

fn area(b: CGRect) -> Area {
    Area { left: b.origin.x, top: b.origin.y, right: b.origin.x + b.size.width, bottom: b.origin.y + b.size.height }
}

struct RealDesk {
    source: CGEventSource,
}

impl MacDesk for RealDesk {
    fn main(&self) -> Area {
        area(CGDisplay::main().bounds())
    }

    fn displays(&self) -> Vec<Area> {
        CGDisplay::active_displays()
            .unwrap_or_default()
            .into_iter()
            .map(|id| area(CGDisplay::new(id).bounds()))
            .collect()
    }

    fn virtual_display(&self) -> Option<Area> {
        CGDisplay::active_displays()
            .unwrap_or_default()
            .into_iter()
            .map(CGDisplay::new)
            .find(|d| d.vendor_number() == crate::macdisplay::VENDOR_ID)
            .map(|d| area(d.bounds()))
    }

    fn pointer(&self) -> (f64, f64) {
        CGEvent::new(self.source.clone()).map(|e| e.location()).map(|p| (p.x, p.y)).unwrap_or_default()
    }

    fn button_held(&self) -> bool {
        const COMBINED_SESSION_STATE: i32 = 0;
        const LEFT: u32 = 0;
        unsafe { CGEventSourceButtonState(COMBINED_SESSION_STATE, LEFT) }
    }

    fn warp(&mut self, to: (f64, f64)) {
        let _ = CGDisplay::warp_mouse_cursor_position(CGPoint::new(to.0, to.1));
    }

    fn move_to(&mut self, to: (f64, f64), delta: (f64, f64), dragging: bool) {
        let kind = if dragging { CGEventType::LeftMouseDragged } else { CGEventType::MouseMoved };
        if let Ok(event) = CGEvent::new_mouse_event(self.source.clone(), kind, CGPoint::new(to.0, to.1), CGMouseButton::Left) {
            event.set_integer_value_field(EventField::MOUSE_EVENT_DELTA_X, delta.0 as i64);
            event.set_integer_value_field(EventField::MOUSE_EVENT_DELTA_Y, delta.1 as i64);
            event.post(CGEventTapLocation::HID);
        }
    }
}

/// Keys, buttons and scrolling, replayed through Lan Mouse's macOS emulation.
struct Emulated {
    emulation: InputEmulation,
    buttons: HashSet<u32>,
}

#[async_trait]
impl Replay for Emulated {
    async fn replay(&mut self, input: Input) {
        if let Input::Button { button, state } = input {
            if state == 0 {
                self.buttons.remove(&button);
            } else {
                self.buttons.insert(button);
            }
        }
        let _ = self.emulation.consume(input.into(), 0).await;
    }

    async fn release_all(&mut self) {
        let _ = self.emulation.release_keys(0).await;
        for button in std::mem::take(&mut self.buttons) {
            let _ = self.emulation.consume(Input::Button { button, state: 0 }.into(), 0).await;
        }
    }

    fn set_language(&mut self, language: &str) {
        crate::sharelog::line(format!("language: {language}"));
        on_main_thread(language.to_string());
    }
}

/// macOS only switches input sources from the app's main thread; sharing runs on its own.
fn on_main_thread(language: String) {
    use std::ffi::c_void;
    extern "C" {
        static _dispatch_main_q: c_void;
        fn dispatch_async_f(queue: *const c_void, context: *mut c_void, work: extern "C" fn(*mut c_void));
    }
    extern "C" fn work(context: *mut c_void) {
        let language = unsafe { Box::from_raw(context as *mut String) };
        select_keyboard_language(&language);
    }
    let context = Box::into_raw(Box::new(language)) as *mut c_void;
    unsafe { dispatch_async_f(&_dispatch_main_q, context, work) };
}

/// Selects the first keyboard input source whose language is `language` ("he", "en"), so the
/// PC's keys type in the PC's language. Does nothing when no such source is enabled.
fn select_keyboard_language(language: &str) {
    use core_foundation::array::{CFArray, CFArrayRef};
    use core_foundation::base::{CFType, TCFType};
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
    use core_foundation::string::{CFString, CFStringRef};
    use std::ffi::c_void;

    #[link(name = "Carbon", kind = "framework")]
    extern "C" {
        static kTISPropertyInputSourceCategory: CFStringRef;
        static kTISCategoryKeyboardInputSource: CFStringRef;
        static kTISPropertyInputSourceIsSelectCapable: CFStringRef;
        static kTISPropertyInputSourceLanguages: CFStringRef;
        fn TISCreateInputSourceList(properties: CFDictionaryRef, include_all_installed: u8) -> CFArrayRef;
        fn TISGetInputSourceProperty(source: *const c_void, key: CFStringRef) -> *const c_void;
        fn TISSelectInputSource(source: *const c_void) -> i32;
    }

    unsafe {
        let filter: CFDictionary<CFString, CFType> = CFDictionary::from_CFType_pairs(&[
            (
                CFString::wrap_under_get_rule(kTISPropertyInputSourceCategory),
                CFString::wrap_under_get_rule(kTISCategoryKeyboardInputSource).as_CFType(),
            ),
            (CFString::wrap_under_get_rule(kTISPropertyInputSourceIsSelectCapable), CFBoolean::true_value().as_CFType()),
        ]);
        let list = TISCreateInputSourceList(filter.as_concrete_TypeRef(), 0);
        if list.is_null() {
            return;
        }
        let sources: CFArray<*const c_void> = CFArray::wrap_under_create_rule(list);
        for source in sources.iter() {
            let source: *const c_void = *source;
            let languages = TISGetInputSourceProperty(source, kTISPropertyInputSourceLanguages);
            if languages.is_null() {
                continue;
            }
            let languages: CFArray<CFString> = CFArray::wrap_under_get_rule(languages as CFArrayRef);
            let first = languages.iter().next().map(|l| crate::language::primary_language(&l.to_string()));
            if first.as_deref() == Some(language) {
                let status = TISSelectInputSource(source);
                crate::sharelog::line(format!("selected the {language} keyboard (status {status})"));
                return;
            }
        }
        crate::sharelog::line(format!("no enabled keyboard for {language}"));
    }
}

type Connection = (
    tokio::sync::mpsc::Receiver<std::io::Result<ToMac>>,
    super::channel::Sender<tokio::io::WriteHalf<tokio::net::TcpStream>>,
);

enum Event {
    Arrived(Connection),
    Message(Option<std::io::Result<ToMac>>),
    Check,
    ClipboardCheck,
    SendPiece,
}

/// The next message on the current connection; never ready without one.
async fn next_message(current: &mut Option<Connection>) -> Option<std::io::Result<ToMac>> {
    match current {
        Some((from_pc, _)) => from_pc.recv().await,
        None => std::future::pending().await,
    }
}

/// Listens until cancelled. Only the trusted PC is answered.
pub async fn run(settings: MacSettings, status: watch::Sender<MouseStatus>, cancel: CancellationToken) {
    let listener = match TcpListener::bind(("0.0.0.0", INPUT_PORT)).await {
        Ok(l) => l,
        Err(e) => {
            status.send_replace(MouseStatus::Failed(format!("port {INPUT_PORT} is in use ({e})")));
            return;
        }
    };
    let emulation = match InputEmulation::new(Some(Backend::MacOs)).await {
        Ok(e) => e,
        Err(_) => {
            status.send_replace(MouseStatus::Failed(
                "Accessibility permission is missing (System Settings → Privacy & Security)".to_string(),
            ));
            return;
        }
    };
    let Ok(source) = CGEventSource::new(CGEventSourceStateID::HIDSystemState) else {
        status.send_replace(MouseStatus::Failed("could not create mouse events".to_string()));
        return;
    };
    let mut desk = RealDesk { source };
    let mut replay = Emulated { emulation, buttons: HashSet::new() };
    replay.emulation.create(0).await;
    status.send_replace(MouseStatus::Running);

    // Connections are accepted and authenticated in the background, so a stranger stalling the
    // handshake, or a PC that vanished without closing its connection, never keeps the real PC
    // out: a newly authenticated connection replaces the current one.
    let (arrived_tx, mut arrived) = tokio::sync::mpsc::channel::<Connection>(4);
    let pc_id = settings.pc_id.clone();
    let token = settings.token.clone();
    tokio::task::spawn_local(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else { continue };
            let _ = stream.set_nodelay(true);
            let (arrived_tx, pc_id, token) = (arrived_tx.clone(), pc_id.clone(), token.clone());
            tokio::task::spawn_local(async move {
                let token_for = |id: &str| (id == pc_id).then(|| token.clone());
                let handshake = tokio::time::timeout(Duration::from_secs(5), mac_handshake(stream, token_for)).await;
                if let Ok(Ok((from_pc, to_pc))) = handshake {
                    let _ = arrived_tx.send((from_pc.into_queue::<ToMac>(), to_pc)).await;
                }
            });
        }
    });

    let mut current: Option<Connection> = None;
    let mut follower = Follower::default();
    let mut last_heard = tokio::time::Instant::now();
    let mut check = tokio::time::interval(super::wire::HEARTBEAT);
    let mut clipboard = if settings.clipboard { crate::clipboard::Local::open() } else { None };
    let mut unpacker = crate::clipboard::Unpacker::new(if clipboard.is_some() {
        crate::clipboard::landing_folder()
    } else {
        std::env::temp_dir()
    });
    let mut watch = crate::clipboard::ChangeWatch::default();
    let mut seen_count = None;
    let mut clip_id = 0u64;
    let mut clip_tick = tokio::time::interval(Duration::from_millis(400));
    let mut outbox: std::collections::VecDeque<ToPc> = std::collections::VecDeque::new();
    loop {
        let event = tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            m = next_message(&mut current) => Event::Message(m),
            Some(connection) = arrived.recv() => Event::Arrived(connection),
            _ = check.tick() => Event::Check,
            _ = clip_tick.tick(), if clipboard.is_some() && current.is_some() => Event::ClipboardCheck,
            _ = std::future::ready(()), if !outbox.is_empty() && current.is_some() => Event::SendPiece,
        };
        let gone = match event {
            Event::Arrived(connection) => {
                crate::sharelog::line("PC connected");
                // Whatever the old connection was doing is over.
                if follower.is_active() {
                    replay.release_all().await;
                }
                follower = Follower::default();
                current = Some(connection);
                last_heard = tokio::time::Instant::now();
                false
            }
            Event::Message(Some(Ok(message))) => {
                last_heard = tokio::time::Instant::now();
                let answer = match message {
                    ToMac::Ping => Some(ToPc::Pong),
                    ToMac::Clip(piece) => {
                        if let (Some(local), Some(content)) = (clipboard.as_mut(), unpacker.feed(piece)) {
                            if local.write(&content) {
                                watch.applied(&content);
                            }
                        }
                        None
                    }
                    other => follower.handle(other, &mut desk, &mut replay).await.map(ToPc::HandBack),
                };
                match (answer, current.as_mut()) {
                    (Some(answer), Some((_, to_pc))) => {
                        !matches!(tokio::time::timeout(Duration::from_secs(1), to_pc.send(&answer)).await, Ok(Ok(())))
                    }
                    _ => false,
                }
            }
            Event::Message(_) => {
                crate::sharelog::line("PC connection ended");
                true
            }
            // A PC that went quiet (asleep, off the network) has let go, whatever it last sent.
            Event::Check => current.is_some() && last_heard.elapsed() > super::wire::SILENCE,
            Event::ClipboardCheck => {
                if let Some(local) = clipboard.as_mut() {
                    let count = local.change_count();
                    if seen_count != Some(count) {
                        seen_count = Some(count);
                        let now = local.read();
                        if watch.local_change(now.as_ref()) {
                            clip_id += 1;
                            if let Some(messages) = now.as_ref().and_then(|c| crate::clipboard::pack(c, clip_id)) {
                                outbox.extend(messages.into_iter().map(ToPc::Clip));
                            }
                        }
                    }
                }
                false
            }
            Event::SendPiece => match (outbox.pop_front(), current.as_mut()) {
                (Some(piece), Some((_, to_pc))) => {
                    !matches!(tokio::time::timeout(Duration::from_secs(1), to_pc.send(&piece)).await, Ok(Ok(())))
                }
                _ => false,
            },
        };
        if gone {
            current = None;
            outbox.clear();
            if follower.is_active() {
                replay.release_all().await;
            }
            follower = Follower::default();
        }
    }
    // Shutting down mid-use: nothing may stay held down.
    if follower.is_active() {
        replay.release_all().await;
    }
    replay.emulation.terminate().await;
}
