//! The display window (PC only): Sidescreen's own window around the Mac's display. Moonlight's
//! stream window is embedded in it as a child, under a 40px title bar drawn by the web view, or
//! filling the whole window when it is full screen.
//!
//! The window is made once, hidden, at start. A session shows it at once (`begin`), with
//! "Starting" in it (the page's placeholder, under where the viewer's window goes), and the
//! viewer's window goes into it when it comes (`attach`). It stays up while viewer windows come
//! and go, and through a restart with new settings; it goes when the display is closed (`close`),
//! when the viewer keeps its own window (the fallback), or when the session ends (`finish`),
//! except after a failure, which it shows with Retry and Close. Never an empty frame: wherever
//! there is no stream, the placeholder shows.

use sidescreen_core::layout::{is_on, monitor_by_number, monitor_number, numbered_monitors};
use sidescreen_core::share::geometry::Area;
use tauri::AppHandle;

pub const LABEL: &str = "display";

/// Where the display window goes, and whether it fills its monitor (`rect` is then the monitor).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placing {
    pub full_screen: bool,
    pub rect: Area,
}

/// Where the display window goes as a session starts: filling the monitor the session `asked`
/// for (its `rect`), or as a window on it, which is the last window if that was on this monitor,
/// else `fresh`. The viewer's windows go in wherever the display window is by then.
#[cfg_attr(not(windows), allow(dead_code))]
fn host_placing(asked: Placing, last_windowed: Option<Area>, fresh: Area) -> Placing {
    if asked.full_screen {
        return asked;
    }
    let rect = last_windowed.filter(|r| is_on(*r, asked.rect)).unwrap_or(fresh);
    Placing { full_screen: false, rect }
}

/// What happens to the display window over a display session.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(not(windows), allow(dead_code))]
enum Step {
    /// A session starts: the window shows at once, "Starting" in it until the viewer's window is
    /// embedded.
    Start,
    /// The settings changed: the session ends, and a new one starts in the same window.
    Restart,
    /// The viewer's window was not embedded (none after 15 s, or the embed failed): it keeps its
    /// own, and the display window goes.
    NoEmbed,
    /// The user closed the display (Close, Alt+F4), or a restart was given up.
    Close,
    /// The session ended by itself; `failed`: the window stays to say why (Retry, Close).
    Ended { failed: bool },
}

/// Whether the display window is up for a display (`shown`: Starting, the stream, or why it
/// failed), and whether it is `held` up through a restart. A viewer window going leaves it as it
/// is: "Starting" shows until the next one is in. Never an empty frame: whatever shows has the
/// placeholder under it.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[cfg_attr(not(windows), allow(dead_code))]
struct Showing {
    shown: bool,
    held: bool,
}

#[cfg_attr(not(windows), allow(dead_code))]
impl Showing {
    fn after(self, step: Step) -> Showing {
        match step {
            Step::Start => Showing { shown: true, held: false },
            Step::Restart => Showing { held: self.shown, ..self },
            Step::NoEmbed | Step::Close => Showing { shown: false, held: false },
            Step::Ended { failed } => Showing { shown: self.shown && (failed || self.held), held: self.held && self.shown },
        }
    }

    /// Whether a session starting brings the window in front: not one held through a restart,
    /// which keeps whatever is in front (it is already there, or the user went elsewhere).
    fn starts_in_front(self) -> bool {
        !(self.shown && self.held)
    }
}

/// What the window in front is, for the display's rules.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(not(windows), allow(dead_code))]
pub enum Front {
    /// The display window (with the stream in it, or Starting).
    Host,
    /// The bar over it (in front while one of its menus is open).
    Bar,
    /// A window of the running viewer the user does not see as one of its own: its stream
    /// window (concealed, in sight, or embedded), or one of the short-lived, invisible windows it
    /// makes while it starts (they take the front for seconds).
    Viewer,
    /// A window of the running viewer the user sees, other than its stream: a dialog (an error,
    /// a pairing prompt).
    ViewerDialog,
    /// No window is in front (for a moment, as the one in front goes).
    Nothing,
    /// Anything else: another app, or Sidescreen's main window.
    Other,
}

/// What the window `front` (of process `front_pid`) is, given the display window `host`, the
/// `bar`, the running viewer's process `viewer` (none while no viewer runs), and whether it is a
/// `dialog` ([`is_dialog`]; only read for the viewer's windows).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn front_of(front: isize, host: Option<isize>, bar: Option<isize>, front_pid: u32, viewer: Option<u32>, dialog: bool) -> Front {
    if front == 0 {
        Front::Nothing
    } else if host == Some(front) {
        Front::Host
    } else if bar == Some(front) {
        Front::Bar
    } else if front_pid != 0 && viewer == Some(front_pid) {
        if dialog {
            Front::ViewerDialog
        } else {
            Front::Viewer
        }
    } else {
        Front::Other
    }
}

/// Whether a window of the viewer is a dialog the user sees: `visible`, `on_a_monitor` (not
/// put out of sight), and not its `stream` window.
#[cfg_attr(not(windows), allow(dead_code))]
fn is_dialog(visible: bool, on_a_monitor: bool, stream: bool) -> bool {
    visible && on_a_monitor && !stream
}

/// Whether the display window takes the front (and the stream the keyboard) once the viewer's
/// window is in it, with `front` in front: from the viewer (its windows took it as it started) or
/// from nothing, and it keeps it when it has it. Never from another app the user went to, nor
/// from a dialog of the viewer (the user reads it), nor from the bar (one of its menus is open;
/// it hands the keyboard back as it closes), and never while `minimized`.
#[cfg_attr(not(windows), allow(dead_code))]
fn takes_front(front: Front, minimized: bool) -> bool {
    !minimized && matches!(front, Front::Host | Front::Viewer | Front::Nothing)
}

/// Whether the display's watchers (the bar's place, the monitors) run, and the bar may show:
/// while a session is `open` and its window shown (Starting or the stream), and between the
/// sessions of a restart, where the window is held up (the bar stays, with the display settings
/// still open in it). Not for the fallback (the viewer keeps its own window), nor for a window
/// left saying why a session failed; with no display in use, nothing runs.
#[cfg_attr(not(windows), allow(dead_code))]
fn watching(open: bool, showing: Showing) -> bool {
    (open || showing.held) && showing.shown
}

/// Whether a restart asked for when the display had been closed `asked_at` times goes on now that
/// it has been closed `closes` times: not when it was closed meanwhile (Close or Alt+F4 while the
/// old session wound down).
#[cfg_attr(not(windows), allow(dead_code))]
fn restart_goes_on(asked_at: u64, closes: u64) -> bool {
    asked_at == closes
}

/// The monitor to be on after the monitors changed: the `current` one while its number is still
/// there, else the lowest number (and `current` when there is none at all).
#[cfg_attr(not(windows), allow(dead_code))]
fn monitor_after_unplug(current: u32, numbers: &[u32]) -> u32 {
    if numbers.contains(&current) {
        current
    } else {
        numbers.iter().copied().min().unwrap_or(current)
    }
}

/// The monitor a full-screen display window should fill once the monitors went from `before` to
/// `now`: the one it fills while that is still there; else the monitor that now has its old
/// number (its own, at a new resolution), or the lowest one ([`monitor_after_unplug`]). `None`
/// with no monitors.
#[cfg_attr(not(windows), allow(dead_code))]
fn monitor_to_fill(filled: Area, before: &[Area], now: &[Area]) -> Option<Area> {
    if now.contains(&filled) {
        return Some(filled);
    }
    // 0 is no monitor's number: a monitor never seen gives way to the lowest one.
    let current = monitor_number(before, &filled).unwrap_or(0);
    let numbers: Vec<u32> = numbered_monitors(now).into_iter().map(|(n, _)| n).collect();
    monitor_by_number(now, monitor_after_unplug(current, &numbers))
}

/// What a change of mode (Full screen, Window) does to the display window.
#[derive(Debug, Clone, Copy, PartialEq)]
enum ModeChange {
    /// The window is not up: only saved. The next session's window goes where the saved mode says.
    SaveOnly,
    /// The window changes now, and that is saved.
    Change,
    /// The window is minimized: where it comes back to changes (and its frame), and that is
    /// saved. It stays minimized.
    ChangeRestored,
}

/// What a change of mode does while the display window is `up` for a display (Starting, the
/// stream, or why it failed: [`Showing`]) and `visible` (`minimized` or not: it is still the
/// display). With no display, and in the fallback (the viewer keeps its own window, which is not
/// moved), the change is only saved.
#[cfg_attr(not(windows), allow(dead_code))]
fn mode_change(up: bool, visible: bool, minimized: bool) -> ModeChange {
    match (up && visible, minimized) {
        (false, _) => ModeChange::SaveOnly,
        (true, false) => ModeChange::Change,
        (true, true) => ModeChange::ChangeRestored,
    }
}

/// The monitor a settings change saves as the display restarts with it. `asked` is the monitor
/// the change carries; when the user `picked` it (a monitor button, even the one already
/// chosen), it wins. Otherwise, while the display is a window, the monitor the window is
/// `window_on` (the user may have dragged it there), so the restart brings it back there rather
/// than to the saved one.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn restart_monitor(asked: Option<u32>, picked: bool, window_on: Option<u32>) -> Option<u32> {
    if picked {
        asked
    } else {
        window_on.or(asked)
    }
}

/// What a full-screen display window that is not exactly over its monitor needs.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Refit {
    Nothing,
    /// Windows sized it (a change of scale): back over its monitor.
    Place(Area),
    /// Moved wholly onto another monitor (Win+Shift+arrow): fill that one.
    Fill(Area),
}

/// The refit for a window at `rect` that fills `filled` (while `full_screen`), decided when the
/// mode changes numbered `seen` and acted on when they are `now`: nothing once the mode changed
/// since (that change placed the window), and nothing for a monitor that went (the poll moves the
/// window, by number).
#[cfg_attr(not(windows), allow(dead_code))]
fn refit_plan(seen: u64, now: u64, full_screen: bool, filled: Option<Area>, monitors: &[Area], rect: Option<Area>) -> Refit {
    if seen != now || !full_screen {
        return Refit::Nothing;
    }
    let Some(filled) = filled.filter(|f| monitors.contains(f)) else { return Refit::Nothing };
    let Some(rect) = rect.filter(|r| *r != filled) else { return Refit::Nothing };
    // Sized for a new scale, it still covers part of its monitor; moved, it is wholly off it.
    let off = rect.right <= filled.left || rect.left >= filled.right || rect.bottom <= filled.top || rect.top >= filled.bottom;
    match monitors.iter().copied().find(|m| is_on(rect, *m)) {
        Some(other) if off && other != filled => Refit::Fill(other),
        _ => Refit::Place(filled),
    }
}

/// Saves the display's mode (and the monitor it fills or is on, when given) without touching the
/// stream, so the display goes on; the main window hears of it. Once per change.
pub fn save_mode(app: &AppHandle, full_screen: bool, monitor: Option<u32>) -> Result<(), String> {
    use sidescreen_core::config::WindowMode;
    use tauri::Manager;
    let shared = app.state::<crate::Shared>();
    let mode = if full_screen { WindowMode::Borderless } else { WindowMode::Windowed };
    match shared.save_display_mode(mode, monitor) {
        Ok(()) => {
            crate::announce(app, &shared);
            Ok(())
        }
        Err(e) => {
            eprintln!("sidescreen: the display's mode was not saved: {e}");
            Err(e)
        }
    }
}

#[cfg(windows)]
pub use imp::*;

#[cfg(windows)]
mod imp {
    use super::{
        front_of, host_placing, is_dialog, mode_change, monitor_to_fill, refit_plan, restart_goes_on, save_mode, takes_front,
        watching, Front, ModeChange, Placing, Refit, Showing, Step, LABEL,
    };
    use serde::Serialize;
    use sidescreen_core::config::Placement;
    use sidescreen_core::layout::{
        child_rect, clamp_into, is_on, monitor_by_number, monitor_label, monitor_number, pulled_window, viewer_frame,
        viewer_monitor, TITLE_BAR,
    };
    use sidescreen_core::session::engines::ViewerHost;
    use sidescreen_core::share::geometry::Area;
    use sidescreen_core::winwindow;
    use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering};
    use std::sync::Mutex;
    use std::time::Duration;
    use tauri::{AppHandle, Emitter, Manager, WebviewWindow, WindowEvent};

    /// Width of a new display window, in logical pixels.
    const WINDOW_WIDTH: f64 = 1280.0;

    /// The display window's handle, read once when it is made. Everything off the main thread
    /// goes through Windows with it: Tauri's getters wait for the main thread, which is blocked
    /// while the app quits (and the session winds down then).
    static HOST_WINDOW: AtomicIsize = AtomicIsize::new(0);

    /// What the display window holds. Locked only briefly, never across a call into Tauri.
    struct Host {
        /// The embedded viewer window, 0 for none.
        child: isize,
        full_screen: bool,
        /// The monitor it fills while full screen.
        filled: Option<Area>,
        /// Where the window was last a window (not full screen), to go back to.
        windowed: Option<Area>,
        /// Counts sessions; an attach for an ended session is refused.
        generation: u64,
        /// The generation of the session that started last ([`begin`]).
        started: u64,
        /// Counts closes ([`close`]): a restart under way gives up after one.
        closes: u64,
        open: bool,
        /// Whether the window is up for a display ([`Showing`]).
        showing: Showing,
    }

    static HOST: Mutex<Host> = Mutex::new(Host {
        child: 0,
        full_screen: false,
        filled: None,
        windowed: None,
        generation: 0,
        started: 0,
        closes: 0,
        open: false,
        showing: Showing { shown: false, held: false },
    });

    /// Whether the window is up for a display: Starting, the stream, or why it failed.
    fn up() -> bool {
        HOST.lock().unwrap().showing.shown
    }

    #[derive(Clone, Serialize)]
    struct DisplayMode {
        full_screen: bool,
        monitor: u32,
    }

    /// Makes the display window, hidden. On the main thread (in setup).
    pub fn create(app: &AppHandle) -> tauri::Result<()> {
        let window = tauri::WebviewWindowBuilder::new(app, LABEL, tauri::WebviewUrl::App("display.html".into()))
            .title("Sidescreen")
            .decorations(false)
            .resizable(true)
            .visible(false)
            .focused(false)
            .theme(Some(tauri::Theme::Dark))
            .background_color(tauri::window::Color(0x20, 0x20, 0x20, 0xff))
            .inner_size(WINDOW_WIDTH, 760.0)
            .min_inner_size(480.0, 320.0)
            .build()?;
        let handle = window.hwnd()?.0 as isize;
        HOST_WINDOW.store(handle, Ordering::Relaxed);
        let _ = APP.set(app.clone());
        let _ = UP.set(tokio::sync::watch::channel(false).0);
        winwindow::watch_child_clicks(handle);
        winwindow::on_child_going(viewer_going);
        winwindow::watch_frame(handle);
        winwindow::watch_moves(handle);
        winwindow::on_move_end(move_ended);
        Ok(())
    }

    /// The app, for [`viewer_going`] and [`move_ended`], which Windows calls with nothing else.
    static APP: std::sync::OnceLock<AppHandle> = std::sync::OnceLock::new();

    /// Whether the display's watchers run ([`watching`]); [`keep_watch`] waits on it.
    static UP: std::sync::OnceLock<tokio::sync::watch::Sender<bool>> = std::sync::OnceLock::new();

    /// Tells the watchers whether a display is in use now. After every change of `open` or
    /// `showing`. Under the host's lock, so the last change is the last word.
    fn sync_watchers() {
        let host = HOST.lock().unwrap();
        let on = watching(host.open, host.showing);
        if let Some(up) = UP.get() {
            up.send_if_modified(|was| std::mem::replace(was, on) != on);
        }
    }

    /// While a display is in use ([`watching`]): places the bar every 250 ms (it follows the
    /// window in front), watches the pointer for it every 40 ms while it shows, and reads the
    /// monitors every second for [`check_monitors`] on the main thread. With no display in use it
    /// waits, doing nothing, after one last placing of the bar (which hides it). While the window
    /// is hidden or minimized, the last monitors it saw are kept, so a monitor unplugged meanwhile
    /// is still found by its old number. Only posts to the main thread; it never waits for it.
    pub async fn keep_watch(app: AppHandle) {
        use std::time::Instant;
        let Some(up) = UP.get() else { return };
        let mut on = up.subscribe();
        let mut known = winwindow::monitors();
        let (mut placed, mut looked) = (Instant::now(), Instant::now());
        loop {
            if !*on.borrow_and_update() {
                let handle = app.clone();
                let _ = app.run_on_main_thread(move || crate::bar::place(&handle));
                if on.wait_for(|on| *on).await.is_err() {
                    return;
                }
            }
            let pointer = crate::bar::watching_pointer();
            tokio::time::sleep(Duration::from_millis(if pointer { 40 } else { 250 })).await;
            crate::bar::watch_pointer(&app);
            if placed.elapsed() >= Duration::from_millis(240) {
                placed = Instant::now();
                let handle = app.clone();
                let _ = app.run_on_main_thread(move || crate::bar::place(&handle));
            }
            if looked.elapsed() < Duration::from_secs(1) {
                continue;
            }
            looked = Instant::now();
            // Mid-change, Windows can list none for a moment.
            let now = winwindow::monitors();
            if now.is_empty() || !showing() {
                continue;
            }
            let before = std::mem::replace(&mut known, now.clone());
            let seen = MODE.load(Ordering::Relaxed);
            let handle = app.clone();
            let _ = app.run_on_main_thread(move || check_monitors(&handle, seen, &before, &now));
        }
    }

    /// A child of the display window is being destroyed. When it is the embedded viewer window,
    /// it is forgotten now; the window stays, "Starting" showing where it was until the next one
    /// is in (or the session's end decides). The viewer's poll (every 250 ms) still detaches too,
    /// as a backstop; by then there is nothing left to do. Main thread (inside the destroy, so it
    /// only reads and tells the page).
    fn viewer_going(gone: isize) {
        let embedded = HOST.lock().unwrap().child;
        if gone == 0 || gone != embedded {
            return;
        }
        if let Some(app) = APP.get() {
            detach(app);
        }
    }

    fn window(app: &AppHandle) -> Option<WebviewWindow> {
        app.get_webview_window(LABEL)
    }

    fn host_window() -> Option<isize> {
        Some(HOST_WINDOW.load(Ordering::Relaxed)).filter(|w| winwindow::is_window(*w))
    }

    /// The display window's handle (for the bar, which sits over it and hands the keyboard back
    /// to it). Never waits for the main thread.
    pub fn window_handle() -> Option<isize> {
        host_window()
    }

    /// What the window `front` is ([`front_of`]), with the `bar`'s window. Reads only: never
    /// waits for the viewer or the main thread.
    pub fn front_now(front: isize, bar: Option<isize>) -> Front {
        let (pid, viewer) = (winwindow::process_of_window(front), winwindow::viewer_process());
        // Only the viewer's windows are looked at further.
        let dialog = pid != 0 && viewer == Some(pid) && {
            let on_a_monitor = winwindow::window_rect(front)
                .is_some_and(|r| sidescreen_core::layout::shows_on(r, &winwindow::monitors()));
            is_dialog(winwindow::is_visible(front), on_a_monitor, winwindow::is_stream_window(front))
        };
        front_of(front, host_window(), bar, pid, viewer, dialog)
    }

    /// A session starts: returns its number, for [`SessionHost`]. The display window shows at
    /// once where the session goes (`full_screen` or not, on the `monitor` chosen, else the one
    /// on the Mac's side), "Starting" in it, and the viewer's window goes into it when it comes.
    /// It comes in front, unless it is held up through a restart (it keeps whatever is in front).
    /// With `SIDESCREEN_NO_EMBED` the viewer keeps its own window and nothing shows. From any
    /// thread: the window is placed on the main thread, never waited for.
    pub fn begin(app: &AppHandle, full_screen: bool, monitor: Option<u32>, placement: Placement, stream: (u32, u32)) -> u64 {
        let embedding = !sidescreen_core::session::engines::embedding_disabled();
        let (generation, in_front) = {
            let mut host = HOST.lock().unwrap();
            host.generation += 1;
            host.started = host.generation;
            host.open = true;
            let in_front = host.showing.starts_in_front();
            host.showing = host.showing.after(if embedding { Step::Start } else { Step::NoEmbed });
            (host.generation, in_front)
        };
        let monitor = viewer_monitor(&winwindow::monitors(), monitor, &placement);
        match monitor.filter(|_| embedding) {
            Some(monitor) => {
                let asked = Placing { full_screen, rect: monitor };
                let handle = app.clone();
                let _ = app.run_on_main_thread(move || show(&handle, generation, asked, stream, in_front));
            }
            None => {
                // No monitor to show it on: the viewer keeps its own window.
                let mut host = HOST.lock().unwrap();
                host.showing = host.showing.after(Step::NoEmbed);
                drop(host);
                hide_now(app);
            }
        }
        sync_watchers();
        generation
    }

    /// Shows the display window for session `generation` where [`host_placing`] says (`asked`:
    /// the session's mode, on its monitor), in front when `in_front`; a window held through a
    /// restart stays as it is in front, minimized or not. Main thread. Also when the session
    /// already ended, as long as its window is to stay up (it says why the session failed), and
    /// no later session started.
    fn show(app: &AppHandle, generation: u64, asked: Placing, stream: (u32, u32), in_front: bool) {
        let wanted = {
            let host = HOST.lock().unwrap();
            host.started == generation && host.showing.shown
        };
        if !wanted {
            return;
        }
        let (Some(window), Some(parent)) = (window(app), host_window()) else { return };
        // A window held through a restart may have been moved since its viewer went.
        remember_windowed();
        let placing = {
            let mut host = HOST.lock().unwrap();
            let placing = host_placing(asked, host.windowed, default_window(asked.rect, stream));
            host.full_screen = placing.full_screen;
            host.filled = placing.full_screen.then_some(placing.rect);
            placing
        };
        mode_changed();
        // A maximized window (never left so) is un-maximized before it loses its frame, or the
        // frame it hangs over the monitor with would become picture.
        if winwindow::is_zoomed(parent) {
            winwindow::restore_to(parent, placing.rect);
        }
        let minimized = winwindow::is_minimized(parent);
        if minimized && !in_front {
            set_frame(app, parent, placing.full_screen, None);
            // It comes back there; moving it now would only move its icon.
            winwindow::set_restored_place(parent, placing.rect);
        } else {
            // Restored straight to its place: never first where it was.
            if minimized {
                // Its frame first, while it is still an icon.
                set_frame(app, parent, placing.full_screen, None);
                winwindow::restore_to(parent, placing.rect);
                let _ = window.unminimize();
                winwindow::place_window(parent, placing.rect);
            } else {
                set_frame(app, parent, placing.full_screen, Some(placing.rect));
            }
            relayout();
        }
        if in_front {
            let _ = window.show();
            let _ = window.set_focus();
        } else if !winwindow::is_visible(parent) {
            let _ = window.show();
        }
        let monitor = if placing.full_screen { Some(placing.rect) } else { monitor_of(placing.rect) };
        eprintln!("sidescreen: the display window shows, starting");
        emit_mode(app, placing.full_screen, monitor);
    }

    fn is_current(generation: u64) -> bool {
        let host = HOST.lock().unwrap();
        host.open && host.generation == generation
    }

    fn monitor_of(r: Area) -> Option<Area> {
        winwindow::monitors().into_iter().find(|m| is_on(r, *m))
    }

    /// Where the display window is, or comes back to while it is minimized (a minimized window
    /// sits at -32000).
    fn place_now(window: isize) -> Option<Area> {
        if winwindow::is_minimized(window) {
            winwindow::normal_rect(window)
        } else {
            winwindow::window_rect(window)
        }
    }

    /// The monitor the display window is on (its centre; minimized, the one it comes back to),
    /// else the first one.
    fn current_monitor() -> Option<Area> {
        host_window()
            .and_then(place_now)
            .and_then(monitor_of)
            .or_else(|| winwindow::monitors().first().copied())
    }

    /// A new window on `monitor`: 1280 wide, shaped like the stream under the title bar, centred,
    /// no bigger than most of the monitor.
    fn default_window(monitor: Area, stream: (u32, u32)) -> Area {
        let scale = winwindow::scale_of_monitor(monitor);
        let w = WINDOW_WIDTH * scale;
        let h = w * stream.1 as f64 / stream.0.max(1) as f64 + TITLE_BAR as f64 * scale;
        viewer_frame(monitor, Area { left: 0.0, top: 0.0, right: w.round(), bottom: h.round() }, false)
    }

    fn emit_mode(app: &AppHandle, full_screen: bool, monitor: Option<Area>) {
        let monitor = monitor.and_then(|m| monitor_number(&winwindow::monitors(), &m)).unwrap_or(0);
        let _ = app.emit("display-mode", DisplayMode { full_screen, monitor });
        // The bar follows at once (to the new monitor, or away from a window), not at its next poll.
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || crate::bar::place(&handle));
    }

    /// Counts the display window's changes of mode or monitor. A refit decided before the latest
    /// change does nothing ([`refit_plan`]): that change placed the window.
    static MODE: AtomicU64 = AtomicU64::new(0);

    fn mode_changed() {
        MODE.fetch_add(1, Ordering::Relaxed);
    }

    /// Full screen: no frame (the window is all picture, edge to edge) and the taskbar out of the
    /// way while the window is in front. Else the frame and the taskbar come back. With `to`, the
    /// window goes there in the same step ([`winwindow::set_filling`]). The taskbar follows the
    /// mode the window has when the main thread gets to it (COM; at once when this is the main
    /// thread, never waited for), so it always ends as the last change left it.
    fn set_frame(app: &AppHandle, parent: isize, full_screen: bool, to: Option<Area>) {
        winwindow::set_filling(parent, full_screen, to);
        let _ = app.run_on_main_thread(move || {
            let full_screen = HOST.lock().unwrap().full_screen;
            winwindow::mark_full_screen(parent, full_screen);
        });
    }

    /// Embeds the viewer's `child` window in the display window, which is already up where the
    /// session put it ([`begin`]), or where the user has moved it since: it goes in there, and
    /// nothing moves. The viewer's windows take the foreground as it starts; when one of them
    /// (or nothing, or the display window) has it once the stream is in, the display window
    /// takes it with the stream getting the keyboard ([`take_front`]); anything else the user
    /// went to meanwhile keeps it. Not on the main thread ([`winwindow::embed`] waits for the
    /// viewer's thread).
    pub fn attach(app: &AppHandle, generation: u64, child: isize) -> Result<(), String> {
        if !is_current(generation) {
            return Err("the display was closed".to_string());
        }
        if !up() {
            return Err("the display window is not up".to_string());
        }
        let parent = host_window().ok_or("the display window has no handle")?;
        // Straight to its place in the window: it never shows anywhere else in it first.
        let full_screen = HOST.lock().unwrap().full_screen;
        let at = winwindow::client_area(parent)
            .filter(|c| c.right > c.left && c.bottom > c.top && !winwindow::is_minimized(parent))
            .map(|c| {
                let (w, h) = ((c.right - c.left) as i32, (c.bottom - c.top) as i32);
                child_rect(w, h, full_screen, winwindow::scale_of_window(parent))
            });
        match at {
            Some(at) => winwindow::embed_at(child, parent, at)?,
            None => winwindow::embed(child, parent)?,
        };
        HOST.lock().unwrap().child = child;
        relayout();
        tell_viewer(app, true);
        // The session ended meanwhile, and its viewer is being stopped: its window is hidden (it
        // never covers a newer session's) and dies with it. The display window is the session
        // end's business ([`finish`], [`close`]).
        if !is_current(generation) {
            winwindow::hide_soon(child);
            let mine = {
                let mut host = HOST.lock().unwrap();
                let mine = host.child == child;
                if mine {
                    host.child = 0;
                }
                mine
            };
            if mine {
                tell_viewer(app, false);
            }
            return Err("the display was closed".to_string());
        }
        take_front(child, parent);
        // Placed again a moment later, through the placer: the viewer may still move or size
        // its own window a moment after it appears, and a move out of sight the concealer asked
        // for just before the embed can land after it (both are done by the viewer's thread,
        // in the order asked, so a later placing undoes it). At 150, 300, 600, 1000 and 2500 ms.
        tauri::async_runtime::spawn(async move {
            for ms in [150, 150, 300, 400, 1500] {
                tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
                if HOST.lock().unwrap().child != child {
                    return;
                }
                relayout();
            }
        });
        Ok(())
    }

    /// The viewer's `child` window is in `parent`: the display window takes the front when
    /// [`takes_front`] says so (from the viewer's windows, which took it as it started, or from
    /// nothing), and the stream gets the keyboard. Decided when the keyboard thread gets to it,
    /// by whoever is in front then, and done there ([`winwindow::take_front`]): never on the main
    /// thread, never waiting for the viewer. From any thread.
    fn take_front(child: isize, parent: isize) {
        let bar = crate::bar::window_handle();
        winwindow::take_front(child, parent, move || {
            if HOST.lock().unwrap().child != child {
                return false;
            }
            let front = front_now(winwindow::foreground_window(), bar);
            let takes = takes_front(front, winwindow::is_minimized(parent));
            if !takes {
                eprintln!("sidescreen: the stream is in; the display window leaves the front to {front:?}");
            }
            takes
        });
    }

    /// Puts the embedded window under the title bar (or over the whole window when full screen),
    /// above the web view and below Tauri's resize edges (hidden when full screen, with or without
    /// a viewer window in it). Through Windows only, so it works from any thread.
    pub fn relayout() {
        let (child, full_screen, up) = {
            let host = HOST.lock().unwrap();
            (host.child, host.full_screen, host.showing.shown)
        };
        let Some(parent) = host_window().filter(|_| up || child != 0) else { return };
        let Some(client) = winwindow::client_area(parent) else { return };
        let (w, h) = ((client.right - client.left) as i32, (client.bottom - client.top) as i32);
        if w <= 0 || h <= 0 || winwindow::is_minimized(parent) {
            return;
        }
        // Never shown: as a window, Windows' own invisible frame takes the resize at the sides and
        // bottom, and Tauri's edges are cut down to the top rows, where they drew the window's
        // plain grey over the title bar (seen live, round 4). The title bar's top edge resizes
        // instead (DisplayTitleBar).
        winwindow::set_resize_borders(parent, false);
        if child != 0 {
            winwindow::place_child(child, child_rect(w, h, full_screen, winwindow::scale_of_window(parent)));
        }
    }

    /// Gives the keyboard to the embedded window, a moment later and never waiting for the viewer
    /// ([`winwindow::focus`]), while the display window is still the one in front then. From any
    /// thread.
    pub fn focus_child() {
        let child = HOST.lock().unwrap().child;
        if child != 0 {
            winwindow::focus(child, move || winwindow::root_in_front(child));
        }
    }

    /// Gives the keyboard to the embedded window while the display window is the one in front
    /// (a change made from the main window or another app leaves their keyboard alone; the bar
    /// hands it back itself as it closes). Main thread.
    fn focus_child_if_in_front(parent: isize) {
        if winwindow::foreground_window() == parent {
            focus_child();
        }
    }

    /// Tells the display window's page, and the bar's (it gives a settings popover kept open
    /// through a restart the keyboard back), whether the viewer's window is in.
    fn tell_viewer(app: &AppHandle, in_it: bool) {
        let _ = app.emit_to(LABEL, "display-viewer", in_it);
        let _ = app.emit_to(crate::bar::LABEL, "display-viewer", in_it);
    }

    /// The window's place right now, unless it is hidden or minimized (a minimized window sits
    /// at -32000).
    fn shown_rect() -> Option<Area> {
        host_window()
            .filter(|w| winwindow::is_visible(*w) && !winwindow::is_minimized(*w))
            .and_then(winwindow::window_rect)
    }

    /// Remembers where the window is while it is up as a window, to come back to.
    fn remember_windowed() {
        let rect = shown_rect();
        let mut host = HOST.lock().unwrap();
        if !host.full_screen && host.showing.shown {
            if let Some(rect) = rect {
                host.windowed = Some(rect);
            }
        }
    }

    /// The embedded window is gone, or about to be. The display window stays as it is, with
    /// "Starting" where the stream was, until the next one is in.
    pub fn detach(app: &AppHandle) {
        remember_windowed();
        let had = {
            let mut host = HOST.lock().unwrap();
            std::mem::replace(&mut host.child, 0) != 0
        };
        if had {
            tell_viewer(app, false);
        }
    }

    /// The display window goes now (a setter: it never waits for the main thread).
    fn hide_now(app: &AppHandle) {
        if let Some(window) = window(app) {
            let _ = window.hide();
        }
    }

    /// The settings changed, and the running display restarts with them: its window stays up
    /// through the session's end, until the next session starts in it. Before the session is
    /// cancelled; a restart given up must [`close`]. Returns what [`restart_still_wanted`] asks
    /// for.
    pub fn restarting() -> u64 {
        let mut host = HOST.lock().unwrap();
        host.showing = host.showing.after(Step::Restart);
        host.closes
    }

    /// Whether the restart that [`restarting`] said `asked_at` for still goes on: not when the
    /// display was closed while the old session wound down (its window went; the new session
    /// must not bring it back).
    pub fn restart_still_wanted(asked_at: u64) -> bool {
        restart_goes_on(asked_at, HOST.lock().unwrap().closes)
    }

    /// The display is closed (Close, Alt+F4, a restart given up): the window goes now, and any
    /// late attach is refused.
    pub fn close(app: &AppHandle) {
        detach(app);
        {
            let mut host = HOST.lock().unwrap();
            host.open = false;
            host.generation += 1;
            host.closes += 1;
            host.showing = host.showing.after(Step::Close);
        }
        sync_watchers();
        hide_now(app);
    }

    /// Session `generation`'s viewer is stopping: any attach still under way is refused, and its
    /// window is forgotten now (it is about to be killed; whether Windows tells of a killed
    /// process's window going is not sure). The window stays, with "Starting" or why it failed
    /// in it; the session's end decides ([`finish`]).
    fn end_viewer(app: &AppHandle, generation: u64) {
        let current = {
            let mut host = HOST.lock().unwrap();
            let current = host.open && host.generation == generation;
            if current {
                host.open = false;
                host.generation += 1;
            }
            current
        };
        if current {
            detach(app);
        }
        sync_watchers();
    }

    /// Session `generation`'s viewer window was not embedded (none came, or the embed failed):
    /// the viewer keeps its own, and the display window goes.
    fn no_embed(app: &AppHandle, generation: u64) {
        let current = {
            let mut host = HOST.lock().unwrap();
            let current = host.open && host.generation == generation;
            if current {
                host.showing = host.showing.after(Step::NoEmbed);
            }
            current
        };
        if current {
            detach(app);
            sync_watchers();
            hide_now(app);
        }
    }

    /// The session is over (`failed`: it ended in a failure). The window goes, unless it shows
    /// why the session failed (with Retry and Close), or it is held for a restart ([`Showing`]).
    pub fn finish(app: &AppHandle, failed: bool) {
        // No viewer is left in it, whatever Windows told: the placeholder shows.
        detach(app);
        let shown = {
            let mut host = HOST.lock().unwrap();
            host.open = false;
            host.generation += 1;
            host.showing = host.showing.after(Step::Ended { failed });
            host.showing.shown
        };
        sync_watchers();
        if !shown {
            hide_now(app);
        }
    }

    /// Whether the display window is full screen, and the monitor it is on; `None` unless a
    /// session shows in it (Starting or the stream) and it shows. For placing the bar over it (a
    /// window left saying why a session failed has its own Retry and Close).
    pub fn is_full_screen(_app: &AppHandle) -> Option<(bool, Area)> {
        let (up, full_screen) = {
            let host = HOST.lock().unwrap();
            (watching(host.open, host.showing), host.full_screen)
        };
        let host = host_window().filter(|w| up && winwindow::is_visible(*w))?;
        let rect = winwindow::window_rect(host)?;
        Some((full_screen, monitor_of(rect)?))
    }

    // Changes of mode and monitor run on the main thread, one at a time: `full_screen` and
    // `windowed` go there, and so do a session's placing as it starts (`show`), the poll's and the
    // scale change's refits, the clamp and the maximize. A change of mode or monitor counts
    // ([`MODE`]). `attach` places nothing: the viewer's window goes in where the window is.

    /// Fills `target` (a monitor): the window becomes exactly the monitor, with no title bar, and
    /// the viewer fills it. `save`: it is the display's mode and monitor from now on (a move
    /// because a monitor went is not saved: the chosen monitor may come back). Main thread.
    fn fill(app: &AppHandle, target: Area, save: bool) -> Result<(), String> {
        let Some(parent) = host_window() else { return Ok(()) };
        let zoomed = winwindow::is_zoomed(parent);
        let minimized = winwindow::is_minimized(parent);
        if zoomed || minimized {
            // Maximized or minimized: the window it was is its restored place.
            let normal = winwindow::normal_rect(parent);
            let mut host = HOST.lock().unwrap();
            if !host.full_screen && host.showing.shown {
                host.windowed = normal.or(host.windowed);
            }
        } else {
            remember_windowed();
        }
        {
            let mut host = HOST.lock().unwrap();
            host.full_screen = true;
            host.filled = Some(target);
        }
        mode_changed();
        // Un-maximized before it loses its frame: the frame a maximized window hangs over the
        // monitor's edges with would otherwise become picture, on the next monitor.
        if zoomed {
            winwindow::restore_to(parent, target);
        }
        if minimized {
            set_frame(app, parent, true, None);
            // It comes back filling the monitor; moving it now would only move its icon.
            winwindow::set_restored_place(parent, target);
        } else {
            set_frame(app, parent, true, Some(target));
            relayout();
        }
        // The keyboard ends on the stream (crossing to the Mac keeps the window in front then).
        focus_child_if_in_front(parent);
        let number = monitor_number(&winwindow::monitors(), &target);
        eprintln!("sidescreen: the display fills monitor {}", monitor_label(number.unwrap_or(0), &target));
        emit_mode(app, true, Some(target));
        if save {
            save_mode(app, true, number)?;
        }
        Ok(())
    }

    /// Runs a change of mode on the main thread, and gives back whether it was saved. A command
    /// is on the main thread already, so the change runs at once and its result is there; from
    /// elsewhere it runs later, unwaited.
    fn on_main_thread(app: &AppHandle, change: impl FnOnce(&AppHandle) -> Result<(), String> + Send + 'static) -> Result<(), String> {
        let (tx, rx) = std::sync::mpsc::channel();
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || {
            let _ = tx.send(change(&handle));
        });
        rx.try_recv().unwrap_or(Ok(()))
    }

    /// Fills `monitor` (by number), or the monitor the window is on, and saves that (once). On the
    /// main thread (at once when called there).
    pub fn full_screen(app: &AppHandle, monitor: Option<u32>) -> Result<(), String> {
        on_main_thread(app, move |app| full_screen_now(app, monitor))
    }

    fn full_screen_now(app: &AppHandle, monitor: Option<u32>) -> Result<(), String> {
        // With no viewer in the window, the display window's own (hidden) monitor is no choice:
        // only what was asked is saved.
        if mode_change_now() == ModeChange::SaveOnly {
            return save_mode(app, true, monitor);
        }
        let target = monitor.and_then(|n| monitor_by_number(&winwindow::monitors(), n)).or_else(current_monitor);
        match target {
            Some(target) => fill(app, target, true),
            None => Ok(()),
        }
    }

    /// Back to a window: where it last was, or a new one on the current monitor; with a
    /// `monitor` (by number), where it last was on that one, or a new one there. Saved (once). On
    /// the main thread (at once when called there).
    pub fn windowed(app: &AppHandle, stream: (u32, u32), monitor: Option<u32>) -> Result<(), String> {
        on_main_thread(app, move |app| {
            let on = monitor.and_then(|n| monitor_by_number(&winwindow::monitors(), n));
            let at = on.map(|m| {
                let last = HOST.lock().unwrap().windowed.filter(|r| is_on(*r, m));
                last.unwrap_or_else(|| default_window(m, stream))
            });
            windowed_now(app, stream, at)
        })
    }

    /// A window `at` that place, else where it last was, else a new one on the current monitor.
    fn windowed_now(app: &AppHandle, stream: (u32, u32), at: Option<Area>) -> Result<(), String> {
        if mode_change_now() == ModeChange::SaveOnly {
            return save_mode(app, false, None);
        }
        let Some(parent) = host_window() else { return Ok(()) };
        let Some(monitor) = at.and_then(monitor_of).or_else(current_monitor) else { return Ok(()) };
        let last = {
            let mut host = HOST.lock().unwrap();
            host.full_screen = false;
            host.filled = None;
            host.windowed.filter(|r| is_on(*r, monitor))
        };
        mode_changed();
        let rect = at.or(last).unwrap_or_else(|| default_window(monitor, stream));
        if winwindow::is_zoomed(parent) {
            winwindow::restore_to(parent, rect);
        }
        if winwindow::is_minimized(parent) {
            set_frame(app, parent, false, None);
            // It comes back as this window; moving it now would only move its icon.
            winwindow::set_restored_place(parent, rect);
        } else {
            set_frame(app, parent, false, Some(rect));
            relayout();
        }
        // The keyboard ends on the stream, as after `fill`.
        focus_child_if_in_front(parent);
        emit_mode(app, false, Some(monitor));
        // The monitor it is on, so a restart (or the next start) opens it there.
        save_mode(app, false, monitor_number(&winwindow::monitors(), &monitor))
    }

    /// The bar over the full-screen window was pulled down: the window becomes a window under the
    /// pointer ([`pulled_window`]: 880 wide, its title bar under the pointer), saved as the mode,
    /// and follows the pointer until the button is let go. Let go against a monitor's top edge,
    /// Windows maximizes it, which is full screen there ([`full_screen_if_maximized`]). With the
    /// button already up, the window just stays where it was put. On the main thread (at once
    /// when called there, as the command is).
    pub fn pull_down(app: &AppHandle, stream: (u32, u32)) {
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || pull_down_now(&handle, stream));
    }

    fn pull_down_now(app: &AppHandle, stream: (u32, u32)) {
        let full_screen = HOST.lock().unwrap().full_screen;
        if !full_screen || !showing() {
            return;
        }
        let Some(parent) = host_window() else { return };
        let Some(cursor) = winwindow::cursor() else { return };
        let monitor = winwindow::monitors().into_iter().find(|m| m.contains(cursor)).or_else(current_monitor);
        let Some(monitor) = monitor else { return };
        let at = (cursor.0.round() as i32, cursor.1.round() as i32);
        let rect = pulled_window(at, stream, winwindow::work_area(monitor), winwindow::scale_of_monitor(monitor));
        // Only the window in front can take the mouse for the move. The bar is in front while one
        // of its menus is open; it hides as the window leaves full screen.
        if winwindow::foreground_window() != parent {
            winwindow::bring_to_front(parent);
        }
        let _ = windowed_now(app, stream, Some(rect));
        eprintln!(
            "sidescreen: the display was pulled down to a {} \u{d7} {} window",
            rect.right - rect.left,
            rect.bottom - rect.top
        );
        // The move starts once the main thread is back at its message loop, not inside the web
        // view's callback this runs in.
        winwindow::begin_move_later(parent);
    }

    pub fn minimize(app: &AppHandle) {
        if let Some(window) = window(app) {
            let _ = window.minimize();
        }
    }

    /// A maximize is being turned into full screen.
    static UNMAXIMIZING: AtomicBool = AtomicBool::new(false);

    /// Maximized (dragged to a monitor's top edge, a double click on the title bar, Win+Up): full
    /// screen on that monitor instead. Later, on the main thread, not inside the window's own
    /// resizing (handing the work to the main thread from the main thread would run it at once).
    fn full_screen_if_maximized(app: &AppHandle) {
        let Some(parent) = host_window() else { return };
        if !winwindow::is_zoomed(parent) || UNMAXIMIZING.swap(true, Ordering::Relaxed) {
            return;
        }
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let handle = app.clone();
            let posted = app.run_on_main_thread(move || {
                if winwindow::is_zoomed(parent) {
                    let _ = full_screen_now(&handle, None);
                }
                UNMAXIMIZING.store(false, Ordering::Relaxed);
            });
            if posted.is_err() {
                UNMAXIMIZING.store(false, Ordering::Relaxed);
            }
        });
    }

    /// Puts a full-screen window back over exactly its monitor when Windows sized it (a change of
    /// scale); moved wholly onto another monitor (Win+Shift+arrow), it fills that one. `seen`:
    /// the [`MODE`] when this was decided on; any change since wins. Main thread.
    fn refit(app: &AppHandle, seen: u64) {
        let (full_screen, filled) = {
            let host = HOST.lock().unwrap();
            (host.full_screen && host.showing.shown, host.filled)
        };
        let Some(parent) = host_window().filter(|w| winwindow::is_visible(*w) && !winwindow::is_minimized(*w)) else {
            return;
        };
        let plan = refit_plan(seen, MODE.load(Ordering::Relaxed), full_screen, filled, &winwindow::monitors(), winwindow::window_rect(parent));
        // Checked again right before acting: a session's window may have been placed meanwhile.
        if MODE.load(Ordering::Relaxed) != seen {
            return;
        }
        match plan {
            Refit::Nothing => {}
            Refit::Place(monitor) => {
                winwindow::place_window(parent, monitor);
                relayout();
            }
            Refit::Fill(monitor) => {
                let _ = fill(app, monitor, true);
            }
        }
    }

    /// What a change of mode does now ([`mode_change`]).
    fn mode_change_now() -> ModeChange {
        let window = host_window();
        let visible = window.is_some_and(winwindow::is_visible);
        let minimized = window.is_some_and(winwindow::is_minimized);
        mode_change(up(), visible, minimized)
    }

    /// The monitor (by number) the display window is on while it is up as a window (minimized:
    /// the one it comes back to). `None` full screen (the monitor it fills is saved as it fills
    /// it) or not up.
    pub fn windowed_monitor() -> Option<u32> {
        let (up, full_screen) = {
            let host = HOST.lock().unwrap();
            (host.showing.shown, host.full_screen)
        };
        if !up || full_screen {
            return None;
        }
        let window = host_window().filter(|w| winwindow::is_visible(*w))?;
        let monitor = place_now(window).and_then(monitor_of)?;
        monitor_number(&winwindow::monitors(), &monitor)
    }

    /// Whether the display window is up and shows right now (not hidden, not minimized).
    fn showing() -> bool {
        up() && host_window().is_some_and(|w| winwindow::is_visible(w) && !winwindow::is_minimized(w))
    }

    /// The monitors went from `before` to `now` (or stayed): a full-screen window whose monitor
    /// went fills [`monitor_to_fill`], one Windows moved or sized goes back ([`refit`]), and a
    /// change is told (`display-mode`). Main thread.
    fn check_monitors(app: &AppHandle, seen: u64, before: &[Area], now: &[Area]) {
        if !showing() {
            return;
        }
        let (full_screen, filled) = {
            let host = HOST.lock().unwrap();
            (host.full_screen, host.filled)
        };
        if let Some(filled) = filled.filter(|_| full_screen && MODE.load(Ordering::Relaxed) == seen) {
            match monitor_to_fill(filled, before, now) {
                Some(target) if target != filled => {
                    let _ = fill(app, target, false);
                    return;
                }
                _ => refit(app, seen),
            }
        }
        if now != before {
            emit_mode(app, full_screen, current_monitor());
        }
    }

    /// The display window's own events (main thread). A move alone changes nothing inside the
    /// window (a move to a monitor with another scale comes as a scale change).
    pub fn on_event(app: &AppHandle, event: &WindowEvent) {
        match event {
            WindowEvent::Resized(_) => {
                relayout();
                full_screen_if_maximized(app);
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                relayout();
                // Windows sizes the window for the new scale after this event: full screen, it
                // goes back over its monitor once that is done, unless the mode changed by then.
                // A window is kept inside its monitor's work area then; while the user drags it,
                // once the drag ends ([`move_ended`]).
                if HOST.lock().unwrap().full_screen {
                    let seen = MODE.load(Ordering::Relaxed);
                    let app = app.clone();
                    tauri::async_runtime::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(200)).await;
                        let handle = app.clone();
                        let _ = app.run_on_main_thread(move || refit(&handle, seen));
                    });
                } else if !winwindow::in_move_loop() {
                    keep_inside_later(app, Duration::from_millis(200));
                }
            }
            WindowEvent::Focused(true) => focus_child(),
            _ => {}
        }
    }

    /// The user ended moving or sizing the display window: it is put back inside its monitor's
    /// work area. Dragged across monitors of another scale, Windows sized it for the new one,
    /// which can take it past that work area (below the taskbar). Main thread, inside the move
    /// loop's end: acted on later.
    fn move_ended(_window: isize, _scaled: bool) {
        if let Some(app) = APP.get() {
            keep_inside_later(app, Duration::ZERO);
        }
    }

    /// [`keep_inside`] `after` that long, on the main thread, unless the mode changes meanwhile.
    fn keep_inside_later(app: &AppHandle, after: Duration) {
        let seen = MODE.load(Ordering::Relaxed);
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(after).await;
            let _ = app.run_on_main_thread(move || keep_inside(seen));
        });
    }

    /// Puts the display window, up as a window, back inside the work area of the monitor it is
    /// on ([`clamp_into`]: at its size if that fits, else shrunk to fit). `seen`: the [`MODE`]
    /// when this was decided on; any change since wins. Main thread.
    fn keep_inside(seen: u64) {
        let (up, full_screen) = {
            let host = HOST.lock().unwrap();
            (host.showing.shown, host.full_screen)
        };
        if !up || full_screen || MODE.load(Ordering::Relaxed) != seen || winwindow::in_move_loop() {
            return;
        }
        let Some(parent) = host_window().filter(|w| {
            winwindow::is_visible(*w) && !winwindow::is_minimized(*w) && !winwindow::is_zoomed(*w)
        }) else {
            return;
        };
        let (Some(rect), Some(visible)) = (winwindow::window_rect(parent), winwindow::visible_rect(parent)) else {
            return;
        };
        // The work area of the monitor nearest its middle.
        let to = clamp_into(rect, visible, winwindow::work_area(visible));
        if to != rect {
            winwindow::place_window(parent, to);
            relayout();
            eprintln!("sidescreen: the display window was put back inside its monitor's work area");
        }
    }

    /// One session's view of the display window, handed to the viewer.
    pub struct SessionHost {
        pub app: AppHandle,
        /// From [`begin`], which showed the window for it.
        pub generation: u64,
    }

    impl ViewerHost for SessionHost {
        fn attach(&self, child: isize) -> Result<(), String> {
            attach(&self.app, self.generation, child)
        }

        fn detach(&self) {
            if is_current(self.generation) {
                detach(&self.app);
            }
        }

        fn fallback(&self) {
            no_embed(&self.app, self.generation);
        }

        fn end(&self) {
            end_viewer(&self.app, self.generation);
        }

        fn area(&self) -> Option<Area> {
            shown_rect()
        }
    }
}

/// Fills a monitor (by number; the current one when `None`). With no display window up (no
/// display, or the fallback, where the viewer keeps its own window and is not moved), it is only
/// saved. Saved once either way.
#[tauri::command]
pub fn display_full_screen(app: AppHandle, monitor: Option<u32>) -> Result<(), String> {
    #[cfg(windows)]
    return full_screen(&app, monitor);
    #[cfg(not(windows))]
    save_mode(&app, true, monitor)
}

/// Back to a window (on `monitor`, by number, when given; else the current one). With no display
/// window up, only saved (as above).
#[tauri::command]
pub fn display_window(app: AppHandle, monitor: Option<u32>) -> Result<(), String> {
    #[cfg(windows)]
    {
        use tauri::Manager;
        let s = app.state::<crate::Shared>().status().settings;
        windowed(&app, (s.width, s.height), monitor)
    }
    #[cfg(not(windows))]
    save_mode(&app, false, monitor)
}

/// The bar was pulled down: a window under the pointer, which follows it until the button is let go.
#[tauri::command]
pub fn display_pull_down(app: AppHandle) {
    #[cfg(windows)]
    {
        use tauri::Manager;
        let s = app.state::<crate::Shared>().status().settings;
        pull_down(&app, (s.width, s.height));
    }
    #[cfg(not(windows))]
    let _ = app;
}

#[tauri::command]
pub fn display_minimize(app: AppHandle) {
    #[cfg(windows)]
    minimize(&app);
    #[cfg(not(windows))]
    let _ = app;
}

#[cfg(test)]
mod tests {
    use super::*;
    use sidescreen_core::share::geometry::Area;

    const M1: Area = Area { left: 0.0, top: 0.0, right: 1920.0, bottom: 1080.0 };
    const M2: Area = Area { left: 1920.0, top: 0.0, right: 4480.0, bottom: 1440.0 };
    const FRESH: Area = Area { left: 100.0, top: 100.0, right: 1380.0, bottom: 860.0 };
    const ON_M2: Area = Area { left: 2000.0, top: 100.0, right: 3000.0, bottom: 700.0 };

    fn asked(full_screen: bool, monitor: Area) -> Placing {
        Placing { full_screen, rect: monitor }
    }

    #[test]
    fn a_session_goes_where_it_asks() {
        assert_eq!(host_placing(asked(true, M1), Some(ON_M2), FRESH), Placing { full_screen: true, rect: M1 });
        assert_eq!(host_placing(asked(false, M1), None, FRESH), Placing { full_screen: false, rect: FRESH });
    }

    #[test]
    fn a_window_comes_back_where_it_last_was_on_the_asked_monitor() {
        assert_eq!(host_placing(asked(false, M2), Some(ON_M2), FRESH), Placing { full_screen: false, rect: ON_M2 });
        // Elsewhere: a fresh window on the asked monitor.
        assert_eq!(host_placing(asked(false, M1), Some(ON_M2), FRESH), Placing { full_screen: false, rect: FRESH });
    }

    fn through(steps: &[Step]) -> Showing {
        steps.iter().fold(Showing::default(), |s, step| s.after(*step))
    }

    #[test]
    fn a_start_shows_the_window_at_once_in_front() {
        assert!(!Showing::default().shown);
        assert!(Showing::default().starts_in_front());
        assert!(through(&[Step::Start]).shown);
    }

    #[test]
    fn a_session_that_ends_hides_the_window() {
        assert!(!through(&[Step::Start, Step::Ended { failed: false }]).shown);
    }

    #[test]
    fn a_failed_session_keeps_the_window_to_say_why() {
        let failed = through(&[Step::Start, Step::Ended { failed: true }]);
        assert!(failed.shown);
        // Retry: the same window starts again, in front.
        assert!(failed.starts_in_front());
        assert!(failed.after(Step::Start).shown);
        // Close.
        assert!(!failed.after(Step::Close).shown);
    }

    #[test]
    fn a_restart_keeps_the_window_and_what_is_in_front() {
        let held = through(&[Step::Start, Step::Restart, Step::Ended { failed: false }]);
        assert!(held.shown);
        assert!(!held.starts_in_front());
        let next = held.after(Step::Start);
        assert!(next.shown);
        // The hold is used up: the next session's end hides the window.
        assert!(!next.after(Step::Ended { failed: false }).shown);
    }

    #[test]
    fn a_restart_given_up_hides_the_window() {
        assert!(!through(&[Step::Start, Step::Restart, Step::Ended { failed: false }, Step::Close]).shown);
        assert!(!through(&[Step::Start, Step::Restart, Step::Close]).shown);
    }

    #[test]
    fn closing_hides_the_window_whatever_comes_after() {
        assert!(!through(&[Step::Start, Step::Close, Step::Ended { failed: true }]).shown);
        assert!(!through(&[Step::Start, Step::Restart, Step::Close, Step::Ended { failed: false }]).shown);
    }

    #[test]
    fn the_fallback_hides_the_window_for_good() {
        // The viewer keeps its own window: no Starting left behind, and no failure over it.
        assert!(!through(&[Step::Start, Step::NoEmbed]).shown);
        assert!(!through(&[Step::Start, Step::NoEmbed, Step::Ended { failed: true }]).shown);
    }

    #[test]
    fn a_restart_of_a_window_that_fell_back_shows_it_in_front() {
        // Nothing shows to be kept, so the new session's window comes as on a start.
        let s = through(&[Step::Start, Step::NoEmbed, Step::Restart, Step::Ended { failed: false }]);
        assert!(!s.shown);
        assert!(s.starts_in_front());
    }

    #[test]
    fn nothing_watches_while_no_display_is_in_use() {
        assert!(!watching(false, Showing::default()));
    }

    #[test]
    fn the_watchers_run_while_a_session_shows_in_the_window() {
        // Starting or streaming.
        assert!(watching(true, through(&[Step::Start])));
    }

    #[test]
    fn nothing_watches_a_window_no_session_is_in_or_a_session_with_no_window() {
        // The fallback: the viewer keeps its own window.
        assert!(!watching(true, through(&[Step::Start, Step::NoEmbed])));
        // A failure on show.
        assert!(!watching(false, through(&[Step::Start, Step::Ended { failed: true }])));
    }

    #[test]
    fn the_watchers_and_the_bar_stay_between_the_sessions_of_a_restart() {
        let between = through(&[Step::Start, Step::Restart, Step::Ended { failed: false }]);
        assert!(watching(false, between));
        // Given up: they go.
        assert!(!watching(false, between.after(Step::Close)));
    }

    #[test]
    fn a_restart_goes_on_unless_the_display_was_closed_meanwhile() {
        assert!(restart_goes_on(3, 3));
        // Close (or Alt+F4) while the old session wound down.
        assert!(!restart_goes_on(3, 4));
    }

    const HOST: isize = 0x100;
    const BAR: isize = 0x200;
    const MAIN: isize = 0x300;
    const MOONLIGHT: isize = 0x400;
    const OURS: u32 = 10;
    const VIEWER: u32 = 20;
    const OTHER_APP: u32 = 30;

    #[test]
    fn the_display_window_and_the_bar_are_known_by_their_windows() {
        assert_eq!(front_of(HOST, Some(HOST), Some(BAR), OURS, Some(VIEWER), false), Front::Host);
        assert_eq!(front_of(BAR, Some(HOST), Some(BAR), OURS, Some(VIEWER), false), Front::Bar);
    }

    #[test]
    fn any_window_of_the_viewer_counts_as_the_viewers() {
        // Its stream window, or one of the short-lived windows it shows while it starts.
        assert_eq!(front_of(MOONLIGHT, Some(HOST), Some(BAR), VIEWER, Some(VIEWER), false), Front::Viewer);
    }

    #[test]
    fn a_window_of_the_viewer_the_user_sees_that_is_not_its_stream_is_a_dialog() {
        // An error or a pairing prompt.
        assert_eq!(front_of(MOONLIGHT, Some(HOST), Some(BAR), VIEWER, Some(VIEWER), true), Front::ViewerDialog);
        assert!(is_dialog(true, true, false));
    }

    #[test]
    fn a_hidden_concealed_or_stream_window_of_the_viewer_is_no_dialog() {
        // Its short-lived windows (not visible), its stream window out of sight, or in sight.
        assert!(!is_dialog(false, true, false));
        assert!(!is_dialog(true, false, true));
        assert!(!is_dialog(true, true, true));
        assert!(!is_dialog(true, false, false));
    }

    #[test]
    fn the_main_window_and_other_apps_are_other() {
        assert_eq!(front_of(MAIN, Some(HOST), Some(BAR), OURS, Some(VIEWER), false), Front::Other);
        assert_eq!(front_of(MAIN, Some(HOST), Some(BAR), OTHER_APP, Some(VIEWER), false), Front::Other);
        assert_eq!(front_of(MAIN, Some(HOST), Some(BAR), OTHER_APP, Some(VIEWER), true), Front::Other);
        // No viewer running: a window of the process that was the viewer is just another app's.
        assert_eq!(front_of(MOONLIGHT, Some(HOST), Some(BAR), VIEWER, None, false), Front::Other);
    }

    #[test]
    fn no_window_in_front_is_nothing_whatever_the_viewer() {
        assert_eq!(front_of(0, Some(HOST), Some(BAR), 0, None, false), Front::Nothing);
        assert_eq!(front_of(0, Some(HOST), Some(BAR), 0, Some(VIEWER), false), Front::Nothing);
        // Not mistaken for the display window or the bar when they have no window.
        assert_eq!(front_of(0, None, None, 0, Some(0), false), Front::Nothing);
    }

    #[test]
    fn the_stream_takes_the_front_back_from_the_viewer_or_from_nothing() {
        assert!(takes_front(Front::Viewer, false));
        assert!(takes_front(Front::Nothing, false));
        // Already in front: the stream just gets the keyboard.
        assert!(takes_front(Front::Host, false));
    }

    #[test]
    fn the_stream_never_takes_the_front_from_another_app_or_the_bar() {
        assert!(!takes_front(Front::Other, false));
        // A menu of the bar is open: it hands the keyboard back as it closes.
        assert!(!takes_front(Front::Bar, false));
        // A dialog of the viewer the user is reading (an error, a pairing prompt).
        assert!(!takes_front(Front::ViewerDialog, false));
    }

    #[test]
    fn a_minimized_display_window_stays_minimized() {
        assert!(!takes_front(Front::Viewer, true));
        assert!(!takes_front(Front::Host, true));
    }

    #[test]
    fn the_monitor_stays_while_it_is_there() {
        assert_eq!(monitor_after_unplug(2, &[1, 2, 3]), 2);
        assert_eq!(monitor_after_unplug(1, &[1]), 1);
    }

    #[test]
    fn an_unplugged_monitor_gives_way_to_the_lowest_number() {
        assert_eq!(monitor_after_unplug(3, &[1, 2]), 1);
        assert_eq!(monitor_after_unplug(2, &[3, 1]), 1);
        // Nothing to move to: it stays.
        assert_eq!(monitor_after_unplug(2, &[]), 2);
    }

    const M3: Area = Area { left: 4480.0, top: 0.0, right: 6400.0, bottom: 1080.0 };

    #[test]
    fn a_full_screen_window_keeps_its_monitor_while_it_is_there() {
        // Monitor 1 went: the one filled is now number 1, but it is the same monitor.
        assert_eq!(monitor_to_fill(M2, &[M1, M2], &[M2]), Some(M2));
        assert_eq!(monitor_to_fill(M2, &[M1, M2], &[M1, M2]), Some(M2));
    }

    #[test]
    fn a_full_screen_window_moves_when_its_monitor_goes() {
        // By number: 2 is still a number, now the monitor that was 3.
        assert_eq!(monitor_to_fill(M2, &[M1, M2, M3], &[M1, M3]), Some(M3));
        assert_eq!(monitor_to_fill(M3, &[M1, M2, M3], &[M1, M2]), Some(M1));
        assert_eq!(monitor_to_fill(M2, &[M1, M2], &[]), None);
    }

    #[test]
    fn a_full_screen_window_follows_its_monitors_new_resolution() {
        let smaller = Area { left: 1920.0, top: 0.0, right: 3840.0, bottom: 1080.0 };
        assert_eq!(monitor_to_fill(M2, &[M1, M2, M3], &[M1, smaller, M3]), Some(smaller));
    }

    const ALL: [Area; 3] = [M1, M2, M3];

    #[test]
    fn a_mode_change_with_the_window_not_up_is_only_saved() {
        // No display, or the fallback (the viewer keeps its own window).
        assert_eq!(mode_change(false, false, false), ModeChange::SaveOnly);
        // Up, but the window hidden (it is about to go).
        assert_eq!(mode_change(true, false, false), ModeChange::SaveOnly);
    }

    #[test]
    fn a_mode_change_while_starting_streaming_or_failed_changes_the_window() {
        assert_eq!(mode_change(true, true, false), ModeChange::Change);
    }

    #[test]
    fn a_mode_change_while_minimized_changes_where_the_window_comes_back() {
        assert_eq!(mode_change(true, true, true), ModeChange::ChangeRestored);
    }

    #[test]
    fn a_restart_keeps_the_monitor_a_window_was_dragged_to() {
        // A resolution change carries the saved monitor 1; the window is on 2 now.
        assert_eq!(restart_monitor(Some(1), false, Some(2)), Some(2));
    }

    #[test]
    fn a_restart_takes_a_monitor_the_user_picked() {
        assert_eq!(restart_monitor(Some(2), true, Some(1)), Some(2));
        // The button of the monitor already chosen, to bring a dragged window back to it.
        assert_eq!(restart_monitor(Some(1), true, Some(2)), Some(1));
    }

    #[test]
    fn a_restart_with_no_window_keeps_the_monitor_asked() {
        // Full screen (its monitor is saved as it fills one), or nothing showing.
        assert_eq!(restart_monitor(Some(1), false, None), Some(1));
        assert_eq!(restart_monitor(None, false, None), None);
    }

    #[test]
    fn a_refit_decided_before_a_mode_change_does_nothing() {
        // display_full_screen(1) ran after the refit looked: the window is off monitor 2 on purpose.
        assert_eq!(refit_plan(4, 5, true, Some(M1), &ALL, Some(M1)), Refit::Nothing);
        assert_eq!(refit_plan(4, 5, true, Some(M2), &ALL, Some(FRESH)), Refit::Nothing);
    }

    #[test]
    fn a_window_that_is_not_full_screen_is_never_refitted() {
        assert_eq!(refit_plan(5, 5, false, Some(M2), &ALL, Some(FRESH)), Refit::Nothing);
        assert_eq!(refit_plan(5, 5, false, None, &ALL, Some(FRESH)), Refit::Nothing);
    }

    #[test]
    fn a_full_screen_window_over_its_monitor_is_left_alone() {
        assert_eq!(refit_plan(5, 5, true, Some(M2), &ALL, Some(M2)), Refit::Nothing);
    }

    #[test]
    fn a_full_screen_window_sized_for_a_new_scale_goes_back_over_its_monitor() {
        let scaled = Area { left: 1920.0, top: 0.0, right: 5760.0, bottom: 2160.0 };
        assert_eq!(refit_plan(5, 5, true, Some(M2), &ALL, Some(scaled)), Refit::Place(M2));
    }

    #[test]
    fn a_full_screen_window_moved_wholly_onto_another_monitor_fills_that_one() {
        let moved = Area { left: 4480.0, top: 0.0, right: 6400.0, bottom: 1080.0 };
        assert_eq!(refit_plan(5, 5, true, Some(M2), &ALL, Some(moved)), Refit::Fill(M3));
    }

    #[test]
    fn a_monitor_that_went_is_not_refitted() {
        assert_eq!(refit_plan(5, 5, true, Some(M2), &[M1, M3], Some(M1)), Refit::Nothing);
    }

    #[test]
    fn a_full_screen_window_on_a_monitor_never_seen_goes_to_the_lowest() {
        assert_eq!(monitor_to_fill(M3, &[M1, M2], &[M1, M2]), Some(M1));
    }
}
