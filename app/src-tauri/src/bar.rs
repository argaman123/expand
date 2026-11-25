//! The bar over the full-screen display (PC only): hidden at first; the whole bar appears at the
//! top centre of the display window's monitor when the pointer comes near, and goes when it
//! leaves (round 3: no tab step). It shows while the display window fills its monitor and it (or
//! the bar) is in front.
//!
//! The bar is a see-through window of its own, always on top, of one fixed size per monitor: it
//! never moves or resizes as the bar opens and closes, so nothing under the pointer jumps. What
//! shows is set by its window region ([`region`]): nothing, the open bar (with room for
//! a tooltip while one shows), or all of it while a menu or popover is open. Outside the region
//! the window is not there at all: clicks reach the Mac's display under it (its menu bar, right
//! there). The page animates inside that region, and asks for it ([`bar_shape`]): a bigger one
//! before it grows, a smaller one once it has shrunk.
//!
//! Whether the pointer is near is the app's to tell ([`watch_pointer`], `bar-near`): the page
//! cannot see the pointer where the window has no region.
//!
//! It never takes the keyboard from the display, except while a menu or popover is open: then it
//! is in front, so keys work in it and a click elsewhere closes it ([`take_keyboard`]).
//!
//! The same window also holds the display settings popover for the display window's title bar
//! ([`bar_settings`]): only the Mac's display is under the 40px title bar's page, so the popover
//! opens here, under its button, over the display. The bar doesn't show meanwhile.

use crate::display::Front;
use expand_core::share::geometry::Area;
use tauri::AppHandle;

pub const LABEL: &str = "bar";

/// Logical sizes. The smallest the open bar can be:
const MIN_BAR: (f64, f64) = (136.0, 26.0);
/// The window: room for the widest bar and a tooltip beside it, and for a menu or popover (the
/// display settings, with a list open under one of them) below it.
const WINDOW: (f64, f64) = (1264.0, 480.0);
/// The open bar, until the page says how big it is:
const OPEN: (f64, f64) = (560.0, 44.0);
/// The most the open bar may ask for:
const OPEN_MAX: (f64, f64) = (1200.0, 160.0);
/// Added to the open bar while a tooltip shows: below it (tooltips open downwards), and on both
/// sides (one under an end button reaches past the bar).
const TIP_ROOM: (f64, f64) = (64.0, 40.0);
/// Where the pointer makes the bar appear: this wide round the top centre, this far from the top
/// edge; once it stayed there a moment ([`DWELL_MS`]), or at once against the top edge
/// ([`TOP_EDGE`]). The whole bar shows then (round 3: there is no tab step).
const REVEAL: (f64, f64) = (376.0, 64.0);
/// How long the pointer stays in [`REVEAL`] before the bar comes (ms): one passing through, going
/// for a tab or a menu at the Mac's top centre, brings nothing.
const DWELL_MS: u64 = 300;
/// The pointer this close to the monitor's top edge (logical px), inside [`REVEAL`]'s width, brings
/// the bar at once, as Windows' auto-hide taskbar does.
const TOP_EDGE: f64 = 2.0;
/// Where it keeps the bar once it appeared (wider and deeper, so it doesn't flicker at the edge).
const KEEP: (f64, f64) = (536.0, 96.0);
/// How long the bar stays after the pointer left [`KEEP`] (ms): the owner found it went too fast
/// (round 4). Coming back meanwhile keeps it.
const LINGER_MS: u64 = 1200;
/// The window while it holds the title bar's settings popover: the popover (340 wide, with a list
/// open under one of its settings), and its shadow's room on the right.
const POPUP: (f64, f64) = (364.0, 480.0);
/// The popover's right edge is this far inside the window's (as in bar.css).
const POPUP_INSET: f64 = 8.0;

/// What of the bar shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shape {
    /// Nothing: the window takes no pointer anywhere.
    Hidden,
    /// The open bar, `width` × `height` (logical) as the page measured it; with room round it
    /// while a `tip` shows.
    Bar { width: f64, height: f64, tip: bool },
    /// The whole window: a menu or popover is open under the bar.
    Menu,
}

impl Shape {
    /// The page's words for it.
    fn parse(shape: &str, width: Option<f64>, height: Option<f64>, tip: bool) -> Option<Shape> {
        Some(match shape {
            "hidden" => Shape::Hidden,
            "bar" => {
                let (width, height) = width.zip(height).unwrap_or(OPEN);
                Shape::Bar { width, height, tip }
            }
            "menu" => Shape::Menu,
            _ => return None,
        })
    }
}

/// The bar window's place, in pixels: [`WINDOW`] at the monitor's `scale` (never wider than the
/// monitor), centred along the top edge of the `monitor`.
#[cfg_attr(not(windows), allow(dead_code))]
fn window_rect(monitor: Area, scale: f64) -> Area {
    let w = (WINDOW.0 * scale).round().min(monitor.right - monitor.left);
    let h = (WINDOW.1 * scale).round().min(monitor.bottom - monitor.top);
    let left = monitor.left + ((monitor.right - monitor.left - w) / 2.0).round();
    Area { left, top: monitor.top, right: left + w, bottom: monitor.top + h }
}

/// The part of a window `size` pixels big (at `scale`) that shows for `shape`, in pixels from
/// the window's top-left corner; `None` for nothing. The page centres the bar in the window, so
/// the region is centred too, a pixel wider on each side for rounding.
#[cfg_attr(not(windows), allow(dead_code))]
fn region(shape: Shape, size: (f64, f64), scale: f64) -> Option<Area> {
    let centred = |w: f64, h: f64| {
        let half = w * scale / 2.0;
        let mid = size.0 / 2.0;
        Area {
            left: ((mid - half).floor() - 1.0).max(0.0),
            top: 0.0,
            right: ((mid + half).ceil() + 1.0).min(size.0),
            bottom: ((h * scale).ceil() + 1.0).min(size.1),
        }
    };
    match shape {
        Shape::Hidden => None,
        Shape::Bar { width, height, tip } => {
            let (w, h) = (width.clamp(MIN_BAR.0, OPEN_MAX.0), height.clamp(MIN_BAR.1, OPEN_MAX.1));
            let (w, h) = if tip { (w + TIP_ROOM.0, h + TIP_ROOM.1) } else { (w, h) };
            Some(centred(w, h))
        }
        Shape::Menu => Some(Area { left: 0.0, top: 0.0, right: size.0, bottom: size.1 }),
    }
}

/// Where the window goes to hold the title bar's settings popover, in pixels at `scale`: under
/// the `button` (screen pixels), the popover's right edge on the button's, kept inside the
/// monitor's `work` area.
#[cfg_attr(not(windows), allow(dead_code))]
fn popup_rect(button: Area, scale: f64, work: Area) -> Area {
    let (w, h) = ((POPUP.0 * scale).round(), (POPUP.1 * scale).round());
    let right = (button.right + (POPUP_INSET * scale).round()).min(work.right);
    let left = (right - w).max(work.left);
    let top = button.bottom.min(work.bottom - h).max(work.top);
    Area { left, top, right: left + w, bottom: top + h }
}

/// Whether the pointer at `at` is near the bar in `window` (at `scale`): in the zone round the top
/// centre that brings the bar ([`REVEAL`]), or, once it was near (`was`), the larger one that
/// keeps it ([`KEEP`]); or on what shows (`showing`, in screen pixels).
#[cfg_attr(not(windows), allow(dead_code))]
fn is_near(at: (f64, f64), window: Area, scale: f64, was: bool, showing: Option<Area>) -> bool {
    if showing.is_some_and(|r| r.contains(at)) {
        return true;
    }
    let (w, h) = if was { KEEP } else { REVEAL };
    let mid = (window.left + window.right) / 2.0;
    let dx = (at.0 - mid).abs();
    let dy = at.1 - window.top;
    dx <= w * scale / 2.0 && (0.0..h * scale).contains(&dy)
}

/// Whether the bar comes, with the pointer at `at` at time `now` (ms): at once on what shows
/// (`showing`, the peek) or pushed against the top edge; else after it stayed [`DWELL_MS`] in the
/// zone (`since`: when it came in, while not yet near). Once near (`was`), [`KEEP`] holds it, and
/// it goes [`LINGER_MS`] after the pointer left that (`since`: when it left, while near). Says
/// whether it is near, and that moment (for the next look).
#[cfg_attr(not(windows), allow(dead_code))]
fn reveal(at: (f64, f64), window: Area, scale: f64, was: bool, showing: Option<Area>, since: Option<u64>, now: u64) -> (bool, Option<u64>) {
    if was {
        if is_near(at, window, scale, true, showing) {
            return (true, None);
        }
        let left = since.unwrap_or(now);
        return if now.saturating_sub(left) >= LINGER_MS { (false, None) } else { (true, Some(left)) };
    }
    if showing.is_some_and(|r| r.contains(at)) {
        return (true, None);
    }
    if !is_near(at, window, scale, false, None) {
        return (false, None);
    }
    if at.1 - window.top < TOP_EDGE * scale {
        return (true, None);
    }
    let since = since.unwrap_or(now);
    if now.saturating_sub(since) >= DWELL_MS {
        (true, None)
    } else {
        (false, Some(since))
    }
}

/// Whether the bar shows: only over a display window that fills its monitor, and then while that
/// window, the bar, or any window of the viewer is in front (as the viewer starts, its own
/// windows take the front for seconds before its stream is in the display window; a dialog of
/// its own, an error, sits over the display), and for the moment no window is in front (as one
/// goes, during a restart: the display settings stay open through it). Open or not: with another
/// app in front, an open bar (and any menu, which could not take the keyboard to be closed) would
/// stay over that app.
#[cfg_attr(not(windows), allow(dead_code))]
fn shows(full_screen: bool, front: Front) -> bool {
    full_screen && in_front(front)
}

/// What the bar window is up as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(windows), allow(dead_code))]
enum Up {
    Hidden,
    /// Holding the title bar's settings popover.
    Popup,
    /// The bar over the full-screen display.
    Bar,
}

/// Whether the bar window coming up says so to the page (`bar-shown`), which then shows the bar by
/// itself for a moment, so it is clear where the bar is: when it is now up as the bar over the
/// full-screen display and was not (it went full screen, also straight from the title bar's
/// popover, or came back in front: alt-tab back, the app activated).
#[cfg_attr(not(windows), allow(dead_code))]
fn comes_up(was: Up, now: Up) -> bool {
    now == Up::Bar && was != Up::Bar
}

/// Whether what is in front leaves the bar (or the title bar's settings popover) up: the display
/// window, the bar, the viewer's windows, or for a moment nothing ([`shows`]).
#[cfg_attr(not(windows), allow(dead_code))]
fn in_front(front: Front) -> bool {
    matches!(front, Front::Host | Front::Bar | Front::Viewer | Front::ViewerDialog | Front::Nothing)
}

#[cfg(windows)]
pub use imp::*;

#[cfg(windows)]
mod imp {
    use super::{comes_up, in_front, popup_rect, region, reveal, shows, window_rect, Shape, Up, LABEL, MIN_BAR};
    use crate::display;
    use expand_core::share::geometry::Area;
    use expand_core::winwindow;
    use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
    use std::sync::Mutex;
    use tauri::{AppHandle, Emitter};

    /// The bar window's handle, read once when it is made. The bar is placed, shown and hidden
    /// through Windows directly: Tauri's setters would also apply its own idea of whether the
    /// window is visible (it never sees the bar shown), and its getters wait for the main thread.
    static BAR_WINDOW: AtomicIsize = AtomicIsize::new(0);

    /// Whether the bar window shows (over a full-screen display); the pointer is watched only then.
    static SHOWN: AtomicBool = AtomicBool::new(false);
    /// What the bar window is up as ([`Up`], as a number), for [`comes_up`].
    static UP_AS: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

    fn up_as(up: Up) -> Up {
        let was = UP_AS.swap(up as u8, Ordering::Relaxed);
        [Up::Hidden, Up::Popup, Up::Bar].into_iter().find(|u| *u as u8 == was).unwrap_or(Up::Hidden)
    }

    struct Bar {
        /// What shows, as the page asked.
        shape: Shape,
        /// A menu or popover is open (the bar has the keyboard).
        menu: bool,
        /// The pointer is near (the page was told).
        near: bool,
        /// When the pointer came into the reveal zone (ms, [`clock`]), while not yet near.
        since: Option<u64>,
        /// The pointer was on the open bar when last looked at.
        pointer_in: bool,
        /// Where the window is, and its monitor's scale.
        placed: Option<(Area, f64)>,
        /// The region last set, once one was.
        region: Option<Option<Area>>,
        /// The window holds the title bar's settings popover, under this button (screen pixels).
        popup: Option<Area>,
    }

    static BAR: Mutex<Bar> = Mutex::new(Bar {
        shape: Shape::Hidden,
        menu: false,
        near: false,
        since: None,
        pointer_in: false,
        placed: None,
        region: None,
        popup: None,
    });

    /// Makes the bar window, hidden. On the main thread (in setup).
    pub fn create(app: &AppHandle) -> tauri::Result<()> {
        let bar = tauri::WebviewWindowBuilder::new(app, LABEL, tauri::WebviewUrl::App("bar.html".into()))
            .title("Expand bar")
            .decorations(false)
            .transparent(true)
            .always_on_top(true)
            .skip_taskbar(true)
            .resizable(false)
            .shadow(false)
            // Clicks don't make it the window in front (`WS_EX_NOACTIVATE`); see `take_keyboard`.
            .focusable(false)
            .focused(false)
            .visible(false)
            .inner_size(MIN_BAR.0, MIN_BAR.1)
            .build()?;
        let handle = bar.hwnd()?.0 as isize;
        BAR_WINDOW.store(handle, Ordering::Relaxed);
        // No show or hide animation, corners or border line: only what the page draws shows.
        winwindow::plain_overlay(handle);
        winwindow::set_region(handle, None);
        BAR.lock().unwrap().region = Some(None);
        Ok(())
    }

    fn bar_window() -> Option<isize> {
        Some(BAR_WINDOW.load(Ordering::Relaxed)).filter(|w| winwindow::is_window(*w))
    }

    /// The bar window's handle (for telling it from what else is in front). Never waits for the
    /// main thread.
    pub fn window_handle() -> Option<isize> {
        bar_window()
    }

    /// Sets the window's region for what shows, unless it is set already. Main thread.
    fn apply_region(bar: isize) {
        let wanted = {
            let b = BAR.lock().unwrap();
            let Some((rect, scale)) = b.placed else { return };
            let wanted = region(b.shape, (rect.right - rect.left, rect.bottom - rect.top), scale);
            if b.region == Some(wanted) {
                return;
            }
            wanted
        };
        winwindow::set_region(bar, wanted);
        BAR.lock().unwrap().region = Some(wanted);
        expand_core::trace::line(|| format!("bar region {wanted:?}"));
    }

    /// Shows the bar window over the full-screen display window, or hides it. Its place changes
    /// only with the monitor (or its scale). Main thread. Every 250 ms while a display is in use
    /// (`display::keep_watch`, which follows the window in front), and when the display window
    /// changes mode or monitor.
    pub fn place(app: &AppHandle) {
        let Some(bar) = bar_window() else { return };
        let front = display::front_now(winwindow::foreground_window(), Some(bar));
        let popup = BAR.lock().unwrap().popup;
        let up = display::is_full_screen(app);
        // Holding the title bar's popover: under its button while the display is up. Else the
        // bar over a full-screen display.
        let placing = match popup {
            Some(button) => up.filter(|_| in_front(front)).map(|(_, monitor)| {
                let scale = winwindow::scale_of_monitor(monitor);
                (popup_rect(button, scale, winwindow::work_area(monitor)), scale)
            }),
            None => up.filter(|(full_screen, _)| shows(*full_screen, front)).map(|(_, monitor)| {
                let scale = winwindow::scale_of_monitor(monitor);
                (window_rect(monitor, scale), scale)
            }),
        };
        let Some((rect, scale)) = placing else {
            SHOWN.store(false, Ordering::Relaxed);
            up_as(Up::Hidden);
            if winwindow::is_visible(bar) {
                winwindow::hide(bar);
                expand_core::trace::line(|| "bar hidden".into());
            }
            reset(app, bar);
            return;
        };
        if winwindow::window_rect(bar) != Some(rect) {
            winwindow::place_window(bar, rect);
        }
        {
            let mut b = BAR.lock().unwrap();
            if b.placed != Some((rect, scale)) {
                b.placed = Some((rect, scale));
                // Pixels at another scale: set again.
                b.region = None;
            }
        }
        apply_region(bar);
        if !winwindow::is_visible(bar) {
            winwindow::show_without_focus(bar);
            expand_core::trace::line(|| format!("bar shown at {rect:?}"));
        }
        SHOWN.store(true, Ordering::Relaxed);
        let now = if popup.is_some() { Up::Popup } else { Up::Bar };
        if comes_up(up_as(now), now) {
            let _ = app.emit_to(LABEL, "bar-shown", ());
        }
    }

    /// A clock for the reveal's dwell (ms since the first look).
    fn clock() -> u64 {
        static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        START.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64
    }

    /// Whether the pointer is watched for the bar: while it shows over a full-screen display
    /// (to bring the bar), not while it holds the title bar's popover (nothing uses it then).
    pub fn watching_pointer() -> bool {
        SHOWN.load(Ordering::Relaxed) && BAR.lock().unwrap().popup.is_none()
    }

    /// Looks where the pointer is, while the bar shows (every 40 ms, off the main thread; only
    /// reads). Tells the page when the pointer comes near or goes (`bar-near`), and, as a
    /// backstop for the page's own tracking, when it left the open bar (`bar-pointer-out`).
    pub fn watch_pointer(app: &AppHandle) {
        if !watching_pointer() {
            return;
        }
        let Some(at) = winwindow::cursor() else { return };
        let (near, left) = {
            let mut b = BAR.lock().unwrap();
            // Holding the title bar's popover, there is no bar to bring.
            if b.popup.is_some() {
                return;
            }
            let Some((rect, scale)) = b.placed else { return };
            let size = (rect.right - rect.left, rect.bottom - rect.top);
            let showing = region(b.shape, size, scale).map(|r| Area {
                left: rect.left + r.left,
                top: rect.top + r.top,
                right: rect.left + r.right,
                bottom: rect.top + r.bottom,
            });
            let (near, since) = reveal(at, rect, scale, b.near, showing, b.since, clock());
            b.since = since;
            let on_bar = matches!(b.shape, Shape::Bar { .. }) && showing.is_some_and(|r| r.contains(at));
            let left = b.pointer_in && !on_bar;
            b.pointer_in = on_bar;
            let changed = b.near != near;
            b.near = near;
            (changed.then_some(near), left)
        };
        if let Some(near) = near {
            let _ = app.emit_to(LABEL, "bar-near", near);
        }
        if left {
            let _ = app.emit_to(LABEL, "bar-pointer-out", ());
        }
    }

    /// The bar went: next time it shows as nothing, until the pointer comes near.
    fn reset(app: &AppHandle, bar: isize) {
        let was = {
            let mut b = BAR.lock().unwrap();
            let was = b.shape != Shape::Hidden || b.menu || b.near || b.popup.is_some();
            b.popup = None;
            b.shape = Shape::Hidden;
            b.menu = false;
            b.near = false;
            b.since = None;
            b.pointer_in = false;
            was
        };
        if was {
            apply_region(bar);
            take_keyboard(bar, false);
            let _ = app.emit_to(LABEL, "bar-closed", ());
        }
    }

    /// The bar's keyboard policy, all in one place. Normally the bar can't be activated, so the
    /// keyboard stays with the Mac's display while it is used. With a menu or popover open it is
    /// put in front: keys (Esc, arrows) work in it, and a click elsewhere takes it out of front,
    /// which the page hears (window `blur`) and closes the menu. Over a full-screen display it is
    /// marked full screen meanwhile so the taskbar stays down while it is the window in front
    /// (not for the title bar's popover). Afterwards the display window gets the keyboard back
    /// (its `Focused` handler gives it to the Mac's display).
    fn take_keyboard(bar: isize, take: bool) {
        if take {
            let over_full_screen = BAR.lock().unwrap().popup.is_none();
            winwindow::set_activatable(bar, true);
            winwindow::mark_full_screen(bar, over_full_screen);
            winwindow::bring_to_front(bar);
        } else {
            winwindow::set_activatable(bar, false);
            winwindow::mark_full_screen(bar, false);
            hand_back(bar);
        }
    }

    /// Gives the keyboard back to the display window if the bar has it: in front, or (clicks in
    /// the bar's web view can take the keyboard without making it the window in front) behind it.
    fn hand_back(bar: isize) {
        let Some(host) = display::window_handle() else { return };
        let front = winwindow::foreground_window();
        if front == bar {
            winwindow::bring_to_front(host);
        } else if front == host {
            display::focus_child();
        }
    }

    /// The page shows `shape` now (or is about to): the window's region follows. Back to the bar
    /// or nothing, any menu is over and the keyboard goes back; nothing also ends holding the
    /// title bar's popover (the window goes). Main thread.
    pub fn set_shape(app: &AppHandle, shape: Shape) {
        let Some(bar) = bar_window() else { return };
        let (closed, popup_over) = {
            let mut b = BAR.lock().unwrap();
            b.shape = shape;
            let closed = !matches!(shape, Shape::Menu) && b.menu;
            if closed {
                b.menu = false;
            }
            let popup_over = shape == Shape::Hidden && b.popup.take().is_some();
            (closed, popup_over)
        };
        apply_region(bar);
        if closed {
            take_keyboard(bar, false);
        }
        if popup_over {
            place(app);
        }
    }

    /// Opens the display settings popover under the display window's title bar button at
    /// `button` (CSS pixels in the display window): the window goes there, all of it showing,
    /// and the page opens the popover (`bar-settings`). Main thread.
    pub fn open_settings(app: &AppHandle, button: Area) {
        let (Some(bar), Some(host)) = (bar_window(), display::window_handle()) else { return };
        let (Some(client), scale) = (winwindow::client_area(host), winwindow::scale_of_window(host)) else { return };
        let on_screen = Area {
            left: client.left + button.left * scale,
            top: client.top + button.top * scale,
            right: client.left + button.right * scale,
            bottom: client.top + button.bottom * scale,
        };
        {
            let mut b = BAR.lock().unwrap();
            b.popup = Some(on_screen);
            b.shape = Shape::Menu;
            b.near = false;
        }
        place(app);
        if winwindow::is_visible(bar) {
            let _ = app.emit_to(LABEL, "bar-settings", ());
        }
    }

    /// A menu or popover opened or closed: the keyboard while it is open (unless the bar was
    /// hidden meanwhile).
    pub fn menu_open(open: bool) {
        let Some(bar) = bar_window() else { return };
        BAR.lock().unwrap().menu = open;
        take_keyboard(bar, open && winwindow::is_visible(bar));
    }
}

/// What of the bar shows (`shape`: "hidden", "bar" or "menu"; for "bar", `width` and
/// `height` are the open bar's own logical size, and `tip` says one of its tooltips shows, which
/// gets room round it). The window's region follows, so only that takes the pointer.
#[tauri::command]
pub fn bar_shape(app: AppHandle, shape: String, width: Option<f64>, height: Option<f64>, tip: Option<bool>) -> Result<(), String> {
    let shape = Shape::parse(&shape, width, height, tip.unwrap_or(false)).ok_or_else(|| format!("unknown shape: {shape}"))?;
    #[cfg(windows)]
    set_shape(&app, shape);
    #[cfg(not(windows))]
    let _ = (app, shape);
    Ok(())
}

/// The display window's title bar settings button (at `left`, `top`, `right`, `bottom`, CSS
/// pixels in the display window) opens the display settings popover under it.
#[tauri::command]
pub fn bar_settings(app: AppHandle, left: f64, top: f64, right: f64, bottom: f64) {
    #[cfg(windows)]
    open_settings(&app, Area { left, top, right, bottom });
    #[cfg(not(windows))]
    let _ = (app, left, top, right, bottom);
}

/// A menu or popover of the bar opened or closed: the bar has the keyboard while it is open.
#[tauri::command]
pub fn bar_menu_open(app: AppHandle, open: bool) {
    let _ = app;
    #[cfg(windows)]
    menu_open(open);
    #[cfg(not(windows))]
    let _ = open;
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIDE: Area = Area { left: 0.0, top: 0.0, right: 2560.0, bottom: 1440.0 };
    const LEFT: Area = Area { left: -1920.0, top: 0.0, right: 0.0, bottom: 1080.0 };
    const SMALL: Area = Area { left: 0.0, top: 0.0, right: 1280.0, bottom: 720.0 };

    #[test]
    fn the_window_sits_at_the_top_centre_of_its_monitor() {
        assert_eq!(window_rect(WIDE, 1.0), Area { left: 648.0, top: 0.0, right: 1912.0, bottom: 480.0 });
        assert_eq!(window_rect(LEFT, 1.0), Area { left: -1592.0, top: 0.0, right: -328.0, bottom: 480.0 });
    }

    #[test]
    fn the_window_grows_with_the_scale_but_never_past_its_monitor() {
        assert_eq!(window_rect(WIDE, 1.5), Area { left: 332.0, top: 0.0, right: 2228.0, bottom: 720.0 });
        assert_eq!(window_rect(SMALL, 1.5), Area { left: 0.0, top: 0.0, right: 1280.0, bottom: 720.0 });
    }

    #[test]
    fn hidden_the_window_takes_no_pointer_anywhere() {
        assert_eq!(region(Shape::Hidden, (1264.0, 480.0), 1.0), None);
    }

    #[test]
    fn a_tiny_bar_still_takes_at_least_136_by_26_in_the_middle() {
        // A pixel more each side, for rounding.
        let tiny = Shape::Bar { width: 10.0, height: 10.0, tip: false };
        assert_eq!(region(tiny, (1264.0, 480.0), 1.0), Some(Area { left: 563.0, top: 0.0, right: 701.0, bottom: 27.0 }));
        // 204 × 39 at 150%.
        assert_eq!(region(tiny, (1896.0, 720.0), 1.5), Some(Area { left: 845.0, top: 0.0, right: 1051.0, bottom: 40.0 }));
    }

    #[test]
    fn the_open_bar_is_exactly_the_bar() {
        // Nothing see-through round it takes the clicks meant for the Mac's menu bar.
        let bar = Shape::Bar { width: 600.0, height: 44.0, tip: false };
        assert_eq!(region(bar, (1264.0, 480.0), 1.0), Some(Area { left: 331.0, top: 0.0, right: 933.0, bottom: 45.0 }));
        let huge = Shape::Bar { width: 5000.0, height: 900.0, tip: false };
        assert_eq!(region(huge, (1264.0, 480.0), 1.0), Some(Area { left: 31.0, top: 0.0, right: 1233.0, bottom: 161.0 }));
    }

    #[test]
    fn a_tooltip_has_room_round_the_bar_only_while_it_shows() {
        let tip = Shape::Bar { width: 600.0, height: 44.0, tip: true };
        assert_eq!(region(tip, (1264.0, 480.0), 1.0), Some(Area { left: 299.0, top: 0.0, right: 965.0, bottom: 85.0 }));
    }

    #[test]
    fn a_menu_has_the_whole_window() {
        assert_eq!(region(Shape::Menu, (1264.0, 480.0), 1.0), Some(Area { left: 0.0, top: 0.0, right: 1264.0, bottom: 480.0 }));
    }

    #[test]
    fn the_page_names_the_shapes() {
        assert_eq!(Shape::parse("hidden", None, None, false), Some(Shape::Hidden));
        assert_eq!(Shape::parse("tab", None, None, false), None, "no tab step (round 3)");
        assert_eq!(Shape::parse("bar", Some(600.0), Some(44.0), true), Some(Shape::Bar { width: 600.0, height: 44.0, tip: true }));
        assert_eq!(Shape::parse("bar", None, None, false), Some(Shape::Bar { width: OPEN.0, height: OPEN.1, tip: false }));
        assert_eq!(Shape::parse("menu", None, None, false), Some(Shape::Menu));
        assert_eq!(Shape::parse("open", None, None, false), None);
    }

    const BAR_WINDOW: Area = Area { left: 648.0, top: 0.0, right: 1912.0, bottom: 480.0 };

    #[test]
    fn the_bar_appears_when_the_pointer_nears_the_top_centre() {
        // The centre is at 1280.
        assert!(is_near((1280.0, 0.0), BAR_WINDOW, 1.0, false, None));
        assert!(is_near((1280.0 + 188.0, 63.0), BAR_WINDOW, 1.0, false, None));
        assert!(!is_near((1280.0, 64.0), BAR_WINDOW, 1.0, false, None), "too low");
        assert!(!is_near((1280.0 + 189.0, 0.0), BAR_WINDOW, 1.0, false, None), "too far to the side");
        assert!(!is_near((100.0, 0.0), BAR_WINDOW, 1.0, false, None), "the Mac's menus, far left");
    }

    #[test]
    fn coming_straight_up_the_bar_shows_before_the_pointer_is_on_it() {
        // The zone reaches below the open bar: the whole bar is there as the pointer arrives.
        let below_the_bar = (1280.0, OPEN.1 + 16.0);
        assert!(is_near(below_the_bar, BAR_WINDOW, 1.0, false, None));
        let bar = Shape::Bar { width: OPEN.0, height: OPEN.1, tip: false };
        assert!(!region(bar, (WINDOW.0, WINDOW.1), 1.0).is_some_and(|r| r.contains((below_the_bar.0 - BAR_WINDOW.left, below_the_bar.1))));
    }

    #[test]
    fn once_near_a_wider_zone_keeps_it() {
        assert!(is_near((1280.0 + 260.0, 80.0), BAR_WINDOW, 1.0, true, None));
        assert!(!is_near((1280.0 + 260.0, 80.0), BAR_WINDOW, 1.0, false, None));
        assert!(!is_near((1280.0, 96.0), BAR_WINDOW, 1.0, true, None));
    }

    #[test]
    fn passing_through_the_zone_brings_nothing() {
        // Going for a tab or a menu at the Mac's top centre: in the zone for under DWELL_MS.
        let (near, since) = reveal((1280.0, 30.0), BAR_WINDOW, 1.0, false, None, None, 1000);
        assert!(!near);
        assert_eq!(since, Some(1000));
        let (near, since) = reveal((1280.0, 30.0), BAR_WINDOW, 1.0, false, None, since, 1000 + DWELL_MS - 40);
        assert!(!near, "not yet");
        assert_eq!(since, Some(1000));
        // Out of the zone again: the dwell starts over.
        assert_eq!(reveal((1280.0, 300.0), BAR_WINDOW, 1.0, false, None, since, 1300), (false, None));
    }

    #[test]
    fn staying_in_the_zone_brings_the_bar() {
        let (_, since) = reveal((1280.0, 30.0), BAR_WINDOW, 1.0, false, None, None, 1000);
        assert_eq!(reveal((1290.0, 20.0), BAR_WINDOW, 1.0, false, None, since, 1000 + DWELL_MS), (true, None));
    }

    #[test]
    fn the_pointer_at_the_top_edge_brings_the_bar_at_once() {
        // As Windows' auto-hide taskbar: pushed against the edge.
        assert_eq!(reveal((1280.0, 0.0), BAR_WINDOW, 1.0, false, None, None, 1000), (true, None));
        assert_eq!(reveal((1280.0 + 150.0, 1.0), BAR_WINDOW, 1.0, false, None, None, 1000), (true, None));
        assert_eq!(reveal((1280.0, 2.0 * 1.5 - 0.5), BAR_WINDOW, 1.5, false, None, None, 1000), (true, None));
        // Not at the far left (the Mac's menus), nor a little below the edge.
        assert_eq!(reveal((100.0, 0.0), BAR_WINDOW, 1.0, false, None, None, 1000), (false, None));
        assert!(!reveal((1280.0, 6.0), BAR_WINDOW, 1.0, false, None, None, 1000).0);
    }

    #[test]
    fn once_near_it_stays_without_a_dwell_and_goes_a_moment_after_leaving_the_keep_zone() {
        assert_eq!(reveal((1280.0 + 260.0, 80.0), BAR_WINDOW, 1.0, true, None, None, 1000), (true, None));
        // Out of the keep zone: it stays a moment (the owner: it went too fast), then goes.
        let (near, since) = reveal((1280.0, 300.0), BAR_WINDOW, 1.0, true, None, None, 1000);
        assert!(near, "gone the moment the pointer left");
        assert_eq!(since, Some(1000));
        assert!(reveal((1280.0, 300.0), BAR_WINDOW, 1.0, true, None, since, 1000 + LINGER_MS - 40).0);
        assert_eq!(reveal((1280.0, 300.0), BAR_WINDOW, 1.0, true, None, since, 1000 + LINGER_MS), (false, None));
    }

    #[test]
    fn coming_back_while_it_lingers_keeps_it_for_good() {
        let (_, since) = reveal((1280.0, 300.0), BAR_WINDOW, 1.0, true, None, None, 1000);
        assert_eq!(reveal((1280.0, 40.0), BAR_WINDOW, 1.0, true, None, since, 1300), (true, None));
    }

    #[test]
    fn the_pointer_on_the_peeking_bar_is_near_at_once() {
        let peek = Area { left: 1000.0, top: 0.0, right: 1560.0, bottom: 45.0 };
        assert_eq!(reveal((1500.0, 40.0), BAR_WINDOW, 1.0, false, Some(peek), None, 1000), (true, None));
    }

    #[test]
    fn the_pointer_on_what_shows_is_near() {
        let menu = Area { left: 648.0, top: 0.0, right: 1912.0, bottom: 480.0 };
        assert!(is_near((700.0, 400.0), BAR_WINDOW, 1.0, true, Some(menu)));
        assert!(!is_near((700.0, 400.0), BAR_WINDOW, 1.0, true, None));
    }

    #[test]
    fn the_zones_grow_with_the_scale() {
        assert!(is_near((1280.0, 95.0), BAR_WINDOW, 1.5, false, None));
        assert!(!is_near((1280.0, 96.0), BAR_WINDOW, 1.5, false, None));
    }

    const WORK: Area = Area { left: 0.0, top: 0.0, right: 2560.0, bottom: 1400.0 };

    #[test]
    fn the_title_bars_popover_opens_under_its_button_lined_up_on_its_right() {
        let button = Area { left: 1000.0, top: 100.0, right: 1046.0, bottom: 140.0 };
        assert_eq!(popup_rect(button, 1.0, WORK), Area { left: 690.0, top: 140.0, right: 1054.0, bottom: 620.0 });
    }

    #[test]
    fn the_title_bars_popover_stays_inside_the_work_area() {
        // Near the left edge, and low on the monitor.
        let button = Area { left: 10.0, top: 1100.0, right: 56.0, bottom: 1140.0 };
        assert_eq!(popup_rect(button, 1.0, WORK), Area { left: 0.0, top: 920.0, right: 364.0, bottom: 1400.0 });
        // At the right edge.
        let button = Area { left: 2500.0, top: 0.0, right: 2560.0, bottom: 40.0 };
        assert_eq!(popup_rect(button, 1.0, WORK).right, 2560.0);
    }

    #[test]
    fn the_bar_shows_over_a_full_screen_display_in_front() {
        assert!(shows(true, Front::Host));
        assert!(shows(true, Front::Bar));
    }

    #[test]
    fn the_bar_stays_while_the_viewer_has_the_front_as_it_starts() {
        // The viewer's own windows take the front for seconds before its stream is in.
        assert!(shows(true, Front::Viewer));
        // A dialog of its own over the display (an error) too: the bar's Close stays at hand.
        assert!(shows(true, Front::ViewerDialog));
    }

    #[test]
    fn the_bar_goes_when_another_window_is_in_front() {
        // Open or with a menu open makes no difference: it would stay over the other app.
        assert!(!shows(true, Front::Other));
    }

    #[test]
    fn the_bar_stays_while_no_window_is_in_front_for_a_moment() {
        // As the old viewer goes in a restart: the display settings stay open.
        assert!(shows(true, Front::Nothing));
    }

    #[test]
    fn the_tab_shows_by_itself_when_the_bar_comes_up_over_the_full_screen_display() {
        // Went full screen, or came back in front.
        assert!(comes_up(Up::Hidden, Up::Bar));
        // Full screen straight from the title bar's popover (its window was up as that).
        assert!(comes_up(Up::Popup, Up::Bar));
    }

    #[test]
    fn the_tab_does_not_show_by_itself_while_the_bar_stays_or_holds_the_popover() {
        assert!(!comes_up(Up::Bar, Up::Bar));
        assert!(!comes_up(Up::Hidden, Up::Popup));
        assert!(!comes_up(Up::Bar, Up::Popup));
        assert!(!comes_up(Up::Bar, Up::Hidden));
    }

    #[test]
    fn the_bar_never_shows_over_a_display_that_is_a_window() {
        assert!(!shows(false, Front::Host));
        assert!(!shows(false, Front::Viewer));
    }
}
