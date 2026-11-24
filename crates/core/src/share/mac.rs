//! The Mac's side of mouse sharing: listens for the trusted PC and follows its mouse and
//! keyboard. The decisions live in `follower`; this file only talks to macOS.

use super::admit::{KnownPc, Slot};
pub use super::admit::MacCommand;
use super::channel::{mac_handshake_claiming, Admit, MacIdentity, INPUT_PORT};
use super::follower::{bounce, Follower, MacDesk, Replay};
use super::geometry::Area;
use super::wire::{Input, ToMac, ToPc};
use crate::mouse::{Lent, MouseStatus};
use async_trait::async_trait;
use core_graphics::display::{CGDisplay, CGPoint, CGRect};
use core_graphics::event::{CGEvent, CGEventTapLocation, CGEventType, CGMouseButton, EventField};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use input_emulation::{Backend, InputEmulation};
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone)]
pub struct MacSettings {
    /// This Mac, as it introduces itself to a PC.
    pub me: MacIdentity,
    /// The PCs this Mac knows (id, pairing token, allowed, name), kept current by the app: a
    /// change is applied to the running server, never a reason to restart it.
    pub pcs: watch::Receiver<Vec<KnownPc>>,
    /// Orders from the app. The server takes the receiver when it starts and gives it back.
    pub commands: Arc<Mutex<Option<mpsc::UnboundedReceiver<MacCommand>>>>,
}

/// The server's identity is the Mac alone: the PC list and the commands reach the running
/// server without restarting it (restarting dropped the connected PC, final review C1).
impl PartialEq for MacSettings {
    fn eq(&self, other: &Self) -> bool {
        self.me == other.me
    }
}

impl Eq for MacSettings {}

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

/// What replays the PC's input on this Mac. It needs the Accessibility permission, which the
/// display does not: it is made when the PC first uses the mouse, not before listening.
struct Replayer {
    desk: RealDesk,
    replay: Emulated,
}

const NO_ACCESSIBILITY: &str = "Accessibility permission is missing (System Settings → Privacy & Security)";

async fn open_input() -> Result<Replayer, String> {
    let source =
        CGEventSource::new(CGEventSourceStateID::HIDSystemState).map_err(|_| "could not create mouse events".to_string())?;
    let emulation = InputEmulation::new(Some(Backend::MacOs)).await.map_err(|_| NO_ACCESSIBILITY.to_string())?;
    let mut replay = Emulated { emulation, buttons: HashSet::new() };
    replay.emulation.create(0).await;
    Ok(Replayer { desk: RealDesk { source }, replay })
}

/// Lets go of everything the follower holds down, when there is anything to let go of.
async fn release(follower: &Follower, input: &mut Option<Replayer>) {
    if let (true, Some(input)) = (follower.is_active(), input.as_mut()) {
        input.replay.release_all().await;
    }
}

/// How often making the input replay is tried again while the PC wants it and it failed.
const INPUT_RETRY: Duration = Duration::from_secs(3);

enum Event {
    Arrived(Connection, String, std::net::IpAddr),
    Pcs,
    Message(Option<std::io::Result<ToMac>>),
    Command(MacCommand),
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

async fn next_command(commands: &mut Option<mpsc::UnboundedReceiver<MacCommand>>) -> Option<MacCommand> {
    match commands {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

/// How long to wait before trying to start again after a start-up failure.
const START_RETRY: Duration = Duration::from_secs(3);

/// Listens until cancelled. Start-up failures (the port is busy) are reported as `Lost(reason)`
/// and tried again, so the reason stays on screen while it retries. A missing Accessibility
/// permission is not one: the PC connects anyway and the problem is reported on the connection.
pub async fn run(settings: MacSettings, status: watch::Sender<MouseStatus>, cancel: CancellationToken) {
    // Put back however this run ends, a crash included, so the next run can take it.
    let mut lent = Lent::take(&*settings.commands);
    let commands = lent.item();
    loop {
        match serve(&settings, &status, &cancel, commands).await {
            Ok(()) => break,
            Err(reason) => {
                status.send_replace(MouseStatus::Lost(reason));
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = tokio::time::sleep(START_RETRY) => {}
                }
            }
        }
    }
}

/// One run of the server: `Err` when it could not start, `Ok` once cancelled.
async fn serve(
    settings: &MacSettings,
    status: &watch::Sender<MouseStatus>,
    cancel: &CancellationToken,
    commands: &mut Option<mpsc::UnboundedReceiver<MacCommand>>,
) -> Result<(), String> {
    let listener = TcpListener::bind(("0.0.0.0", INPUT_PORT))
        .await
        .map_err(|e| format!("port {INPUT_PORT} is in use ({e})"))?;
    serve_on(listener, settings, status, cancel, commands).await;
    Ok(())
}

/// The server on a listener already bound (the Mac's is always `INPUT_PORT`; tests use another),
/// until cancelled.
async fn serve_on(
    listener: TcpListener,
    settings: &MacSettings,
    status: &watch::Sender<MouseStatus>,
    cancel: &CancellationToken,
    commands: &mut Option<mpsc::UnboundedReceiver<MacCommand>>,
) {
    // Made when the PC first uses the mouse.
    let mut input: Option<Replayer> = None;
    let mut input_tried: Option<tokio::time::Instant> = None;
    status.send_replace(MouseStatus::Connecting);

    // Connections are accepted and authenticated in the background, so a stranger stalling the
    // handshake, or a PC that vanished without closing its connection, never keeps the real PC
    // out: a newly authenticated connection from the connected PC replaces its old one.
    let (arrived_tx, mut arrived) = tokio::sync::mpsc::channel::<(Connection, String, std::net::IpAddr)>(4);
    let mut pcs = settings.pcs.clone();
    let slot = Rc::new(RefCell::new(Slot::new(pcs.borrow_and_update().clone())));
    let me = settings.me.clone();
    let door = slot.clone();
    let latest = settings.pcs.clone();
    let acceptor = tokio::task::spawn_local(async move {
        loop {
            let Ok((stream, remote)) = listener.accept().await else { continue };
            let _ = stream.set_nodelay(true);
            let (arrived_tx, me, door, latest) = (arrived_tx.clone(), me.clone(), door.clone(), latest.clone());
            tokio::task::spawn_local(async move {
                // The door only checks; the slot is claimed once the PC has proved it holds
                // the token, before its connection reaches the loop, so a second PC proving
                // itself meanwhile is refused as busy.
                let claimed: Rc<RefCell<Option<String>>> = Rc::default();
                let ask = |id: &str| -> Admit {
                    // The newest list, so a PC that has just paired is let in at once.
                    door.borrow_mut().set_pcs(latest.borrow().clone());
                    door.borrow().check(id)
                };
                let take = |id: &str| -> Result<(), super::channel::Refusal> {
                    door.borrow_mut().set_pcs(latest.borrow().clone());
                    let fresh = door.borrow_mut().claim(id)?;
                    if fresh {
                        *claimed.borrow_mut() = Some(id.to_string());
                    }
                    Ok(())
                };
                let handshake =
                    tokio::time::timeout(Duration::from_secs(5), mac_handshake_claiming(stream, &me, ask, take)).await;
                match handshake {
                    Ok(Ok(((from_pc, to_pc), id))) => {
                        if arrived_tx.send(((from_pc.into_queue::<ToMac>(), to_pc), id.clone(), remote.ip())).await.is_err() {
                            if let Some(id) = claimed.borrow_mut().take() {
                                door.borrow_mut().abandon(&id);
                            }
                        }
                    }
                    _ => {
                        if let Some(id) = claimed.borrow_mut().take() {
                            door.borrow_mut().abandon(&id);
                        }
                    }
                }
            });
        }
    });

    let mut current: Option<Connection> = None;
    let mut follower = Follower::default();
    let mut last_heard = tokio::time::Instant::now();
    // Slow while nobody is connected or the PC uses nothing, from the `Uses` it last sent: the
    // PC keeps the same pace, so neither gives up on the other early.
    let mut pace = slot.borrow().pace();
    let mut check = tokio::time::interval(pace.heartbeat);
    let mut clipboard = crate::clipboard::Local::open();
    let mut unpacker = crate::clipboard::Unpacker::new(crate::clipboard::landing_folder());
    let mut watch = crate::clipboard::ChangeWatch::default();
    let mut seen_count = None;
    let mut clip_id = 0u64;
    let mut clip_tick = tokio::time::interval(Duration::from_millis(400));
    let mut outbox: std::collections::VecDeque<ToPc> = std::collections::VecDeque::new();
    loop {
        let clipboard_on = clipboard.is_some() && slot.borrow().clipboard_allowed();
        let now_pace = slot.borrow().pace();
        if now_pace != pace {
            pace = now_pace;
            check = tokio::time::interval_at(tokio::time::Instant::now() + pace.heartbeat, pace.heartbeat);
        }
        let event = tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            m = next_message(&mut current) => Event::Message(m),
            Some(command) = next_command(commands) => Event::Command(command),
            Some((connection, id, from)) = arrived.recv() => Event::Arrived(connection, id, from),
            Ok(()) = pcs.changed() => Event::Pcs,
            _ = check.tick() => Event::Check,
            _ = clip_tick.tick(), if clipboard_on => Event::ClipboardCheck,
            _ = std::future::ready(()), if !outbox.is_empty() && current.is_some() => Event::SendPiece,
        };
        let gone = match event {
            Event::Arrived(connection, id, from) => {
                crate::sharelog::line("PC connected");
                slot.borrow_mut().arrived(&id);
                slot.borrow_mut().came_from(from);
                // Whatever the old connection was doing is over.
                release(&follower, &mut input).await;
                follower = Follower::default();
                outbox.clear();
                current = Some(connection);
                last_heard = tokio::time::Instant::now();
                status.send_replace(slot.borrow().status());
                false
            }
            Event::Pcs => {
                slot.borrow_mut().set_pcs(pcs.borrow_and_update().clone());
                // A renamed PC shows its new name.
                status.send_replace(slot.borrow().status());
                false
            }
            Event::Command(command) => {
                let cut = slot.borrow_mut().command(command);
                if let (Some(cut), Some((_, to_pc))) = (cut, current.as_mut()) {
                    // Restart says nothing: the PC takes it for a lost network and comes back.
                    if let Some(farewell) = cut.farewell() {
                        let _ = tokio::time::timeout(Duration::from_secs(1), to_pc.send(&farewell)).await;
                    }
                    let _ = tokio::time::timeout(Duration::from_secs(1), to_pc.close()).await;
                    crate::sharelog::line(match cut {
                        super::admit::Cut::Disconnect => "told the PC to disconnect",
                        super::admit::Cut::Drop => "dropped the PC's connection to restart it",
                    });
                }
                cut.is_some()
            }
            Event::Message(Some(Ok(message))) => {
                last_heard = tokio::time::Instant::now();
                let mut over = false;
                let answer = match message {
                    ToMac::Ping => Some(ToPc::Pong),
                    ToMac::Bye => {
                        over = true;
                        None
                    }
                    ToMac::Uses { mouse, clipboard: clip, display, ctrl_as_cmd } => {
                        let went_off = slot.borrow_mut().set_uses(crate::mouse::Uses { mouse, clipboard: clip, display, ctrl_as_cmd });
                        if went_off {
                            // Mouse off stops everything on the connection.
                            release(&follower, &mut input).await;
                            follower = Follower::default();
                            outbox.clear();
                        }
                        if slot.borrow().wants_input() && input.is_none() {
                            input_tried = Some(tokio::time::Instant::now());
                            match open_input().await {
                                Ok(made) => {
                                    input = Some(made);
                                    slot.borrow_mut().set_problem(None);
                                }
                                Err(why) => slot.borrow_mut().set_problem(Some(why)),
                            }
                        }
                        status.send_replace(slot.borrow().status());
                        None
                    }
                    ToMac::Latency { ms } => {
                        if slot.borrow_mut().set_latency(ms) {
                            status.send_replace(slot.borrow().status());
                        }
                        None
                    }
                    ToMac::Clip(piece) => {
                        if slot.borrow().clipboard_allowed() {
                            if let (Some(local), Some(content)) = (clipboard.as_mut(), unpacker.feed(piece)) {
                                if local.write(&content) {
                                    watch.applied(&content);
                                }
                            }
                        }
                        None
                    }
                    other if slot.borrow().replay_allowed() => match input.as_mut() {
                        Some(input) => follower.handle(other, &mut input.desk, &mut input.replay).await.map(ToPc::HandBack),
                        // No permission: the pointer goes straight back to the PC, and anything
                        // else it sends goes nowhere.
                        None => bounce(&other).map(ToPc::HandBack),
                    },
                    _ => None,
                };
                match (answer, current.as_mut()) {
                    (Some(answer), Some((_, to_pc))) => {
                        over || !matches!(tokio::time::timeout(Duration::from_secs(1), to_pc.send(&answer)).await, Ok(Ok(())))
                    }
                    _ => over,
                }
            }
            Event::Message(_) => {
                crate::sharelog::line("PC connection ended");
                true
            }
            // A PC that went quiet (asleep, off the network) has let go, whatever it last sent.
            Event::Check => {
                // A permission granted meanwhile takes effect without reconnecting.
                let due = input_tried.is_none_or(|at| at.elapsed() >= INPUT_RETRY);
                if slot.borrow().wants_input() && input.is_none() && due {
                    input_tried = Some(tokio::time::Instant::now());
                    if let Ok(made) = open_input().await {
                        input = Some(made);
                        slot.borrow_mut().set_problem(None);
                        status.send_replace(slot.borrow().status());
                    }
                }
                current.is_some() && last_heard.elapsed() > pace.silence
            }
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
            slot.borrow_mut().left();
            outbox.clear();
            release(&follower, &mut input).await;
            follower = Follower::default();
            // Back to waiting for a PC.
            status.send_replace(slot.borrow().status());
        }
    }
    // Shutting down mid-use: nothing may stay held down.
    release(&follower, &mut input).await;
    if let Some(mut input) = input {
        input.replay.emulation.terminate().await;
    }
    // Free the port for the next start.
    acceptor.abort();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::share::channel::pc_handshake;
    use std::net::Ipv4Addr;

    const TOKEN: &str = "token";

    type ToMacHalf = super::super::channel::Sender<tokio::io::WriteHalf<tokio::net::TcpStream>>;

    struct Server {
        port: u16,
        status: watch::Receiver<MouseStatus>,
        commands: mpsc::UnboundedSender<MacCommand>,
        cancel: CancellationToken,
        _pcs: watch::Sender<Vec<KnownPc>>,
    }

    /// The Mac's server on a port of its own, knowing one PC (`pc-1`). Call inside a LocalSet.
    async fn server() -> Server {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (pcs_tx, pcs) = watch::channel(vec![("pc-1".to_string(), TOKEN.to_string(), true, "Desk".to_string())]);
        let (commands, rx) = mpsc::unbounded_channel();
        let settings = MacSettings {
            me: MacIdentity { id: "mac-1".into(), name: "Studio".into() },
            pcs,
            commands: Arc::new(Mutex::new(None)),
        };
        let (status_tx, status) = watch::channel(MouseStatus::Off);
        let cancel = CancellationToken::new();
        let stop = cancel.clone();
        tokio::task::spawn_local(async move {
            let mut rx = Some(rx);
            serve_on(listener, &settings, &status_tx, &stop, &mut rx).await;
        });
        Server { port, status, commands, cancel, _pcs: pcs_tx }
    }

    async fn wait_for(rx: &mut watch::Receiver<MouseStatus>, want: impl Fn(&MouseStatus) -> bool) {
        tokio::time::timeout(Duration::from_secs(5), rx.wait_for(|s| want(s))).await.expect("status in time").unwrap();
    }

    /// A PC connected as `pc-1` with the mouse off, so nothing is replayed.
    async fn connect(port: u16) -> (mpsc::Receiver<std::io::Result<ToPc>>, ToMacHalf) {
        let stream = tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await.unwrap();
        let ((from_mac, mut to_mac), _) = pc_handshake(stream, "pc-1", TOKEN).await.unwrap();
        to_mac.send(&ToMac::Uses { mouse: false, clipboard: false, display: true, ctrl_as_cmd: false }).await.unwrap();
        (from_mac.into_queue::<ToPc>(), to_mac)
    }

    /// Everything the Mac says until the connection closes.
    async fn until_closed(from_mac: &mut mpsc::Receiver<std::io::Result<ToPc>>) -> Vec<ToPc> {
        tokio::time::timeout(Duration::from_secs(3), async {
            let mut heard = Vec::new();
            while let Some(Ok(m)) = from_mac.recv().await {
                heard.push(m);
            }
            heard
        })
        .await
        .expect("the connection closed in time")
    }

    // Plan 4, task 2: Restart ends the connection without ToPc::Disconnect, and the PC may
    // come straight back.
    #[tokio::test]
    async fn restart_drops_the_pc_without_telling_it_to_disconnect() {
        tokio::task::LocalSet::new()
            .run_until(async {
                let mut mac = server().await;
                let (mut from_mac, _to_mac) = connect(mac.port).await;
                wait_for(&mut mac.status, |s| matches!(s, MouseStatus::Connected { uses: Some(_), .. })).await;
                mac.commands.send(MacCommand::Restart).unwrap();
                let heard = until_closed(&mut from_mac).await;
                assert!(!heard.contains(&ToPc::Disconnect), "{heard:?}");
                wait_for(&mut mac.status, |s| *s == MouseStatus::Connecting).await;
                let (_from_mac, _to_mac) = connect(mac.port).await;
                wait_for(&mut mac.status, |s| matches!(s, MouseStatus::Connected { .. })).await;
                mac.cancel.cancel();
            })
            .await;
    }

    #[tokio::test]
    async fn disconnect_tells_the_pc_before_closing() {
        tokio::task::LocalSet::new()
            .run_until(async {
                let mut mac = server().await;
                let (mut from_mac, _to_mac) = connect(mac.port).await;
                wait_for(&mut mac.status, |s| matches!(s, MouseStatus::Connected { uses: Some(_), .. })).await;
                mac.commands.send(MacCommand::Disconnect).unwrap();
                let heard = until_closed(&mut from_mac).await;
                assert_eq!(heard.last(), Some(&ToPc::Disconnect), "{heard:?}");
                mac.cancel.cancel();
            })
            .await;
    }

    // Owner 2026-11-23: a PC that uses nothing pings every 15 s; the Mac waits 45 s for it. The
    // 20 s take in the Mac's 15 s check, which must not find 15 s of quiet too long.
    #[tokio::test]
    async fn an_idle_pc_outlasts_the_fast_silence() {
        tokio::task::LocalSet::new()
            .run_until(async {
                let mut mac = server().await;
                let stream = tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, mac.port)).await.unwrap();
                let ((from_mac, mut to_mac), _) = pc_handshake(stream, "pc-1", TOKEN).await.unwrap();
                let mut from_mac = from_mac.into_queue::<ToPc>();
                to_mac.send(&ToMac::Uses { mouse: false, clipboard: false, display: false, ctrl_as_cmd: false }).await.unwrap();
                to_mac.send(&ToMac::Ping).await.unwrap();
                assert_eq!(from_mac.recv().await.unwrap().unwrap(), ToPc::Pong);
                wait_for(&mut mac.status, |s| matches!(s, MouseStatus::Connected { uses: Some(_), .. })).await;
                tokio::time::pause();
                tokio::time::sleep(Duration::from_secs(20)).await;
                tokio::time::resume();
                assert!(matches!(*mac.status.borrow(), MouseStatus::Connected { .. }), "{:?}", *mac.status.borrow());
                mac.cancel.cancel();
            })
            .await;
    }

    // While something is in use the fast pair applies: 3 s of silence and the PC is gone.
    #[tokio::test]
    async fn a_busy_pc_that_goes_quiet_is_let_go() {
        tokio::task::LocalSet::new()
            .run_until(async {
                let mut mac = server().await;
                let (_from_mac, _to_mac) = connect(mac.port).await;
                wait_for(&mut mac.status, |s| matches!(s, MouseStatus::Connected { uses: Some(_), .. })).await;
                let quiet = std::time::Instant::now();
                tokio::time::timeout(Duration::from_secs(6), mac.status.wait_for(|s| *s == MouseStatus::Connecting))
                    .await
                    .expect("let go on the fast pair")
                    .unwrap();
                assert!(quiet.elapsed() >= Duration::from_secs(2), "too soon: {:?}", quiet.elapsed());
                mac.cancel.cancel();
            })
            .await;
    }

    // Plan 4, task 2: the Mac shows the latency the PC reports.
    #[tokio::test]
    async fn the_latency_the_pc_reports_is_the_macs_rtt() {
        tokio::task::LocalSet::new()
            .run_until(async {
                let mut mac = server().await;
                let (_from_mac, mut to_mac) = connect(mac.port).await;
                wait_for(&mut mac.status, |s| matches!(s, MouseStatus::Connected { rtt_ms: None, .. })).await;
                to_mac.send(&ToMac::Latency { ms: 14 }).await.unwrap();
                wait_for(&mut mac.status, |s| matches!(s, MouseStatus::Connected { rtt_ms: Some(14), .. })).await;
                mac.cancel.cancel();
            })
            .await;
    }
}
