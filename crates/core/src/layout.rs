//! Where the viewer window goes on the PC: on the monitor next to the Mac. Pure functions; the
//! Windows calls live in `winwindow`.

use crate::config::Placement;
use crate::share::geometry::Area;
use crate::side::Side;

/// The monitor furthest toward the Mac (the one the Mac sits beside).
pub fn monitor_toward(monitors: &[Area], mac_side: Side) -> Option<Area> {
    let reach = |m: &Area| match mac_side {
        Side::Right => m.right,
        Side::Left => -m.left,
        Side::Bottom => m.bottom,
        Side::Top => -m.top,
    };
    monitors.iter().copied().max_by(|a, b| reach(a).total_cmp(&reach(b)))
}

/// The monitors with their 1-based numbers: left to right, then top to bottom.
pub fn numbered_monitors(monitors: &[Area]) -> Vec<(u32, Area)> {
    let mut sorted = monitors.to_vec();
    sorted.sort_by(|a, b| a.left.total_cmp(&b.left).then(a.top.total_cmp(&b.top)));
    sorted.into_iter().enumerate().map(|(i, m)| (i as u32 + 1, m)).collect()
}

/// The 1-based number of `m`: monitors are numbered left to right, then top to bottom.
pub fn monitor_number(monitors: &[Area], m: &Area) -> Option<u32> {
    numbered_monitors(monitors).into_iter().find(|(_, x)| x == m).map(|(n, _)| n)
}

pub fn monitor_by_number(monitors: &[Area], n: u32) -> Option<Area> {
    numbered_monitors(monitors).into_iter().find(|(i, _)| *i == n).map(|(_, m)| m)
}

/// The number of the monitor a Mac placed under one monitor (Bottom with a number) sits under:
/// that monitor while it is there, else the lowest-numbered one present (an unplugged monitor),
/// as the window draws it. `None` for any other placement, or with no monitors.
pub fn under_monitor(monitors: &[Area], placement: &Placement) -> Option<u32> {
    let (Side::Bottom, Some(n)) = (placement.side, placement.monitor) else { return None };
    let numbered = numbered_monitors(monitors);
    numbered.iter().find(|(i, _)| *i == n).or(numbered.first()).map(|(i, _)| *i)
}

/// The monitor the viewer opens on: the stream's chosen one if it exists, else the one the Mac
/// sits under, else the one toward the Mac.
pub fn viewer_monitor(monitors: &[Area], stream_monitor: Option<u32>, placement: &Placement) -> Option<Area> {
    stream_monitor
        .and_then(|n| monitor_by_number(monitors, n))
        .or_else(|| under_monitor(monitors, placement).and_then(|n| monitor_by_number(monitors, n)))
        .or_else(|| monitor_toward(monitors, placement.side))
}

/// Whether the pointer may go to the Mac from `cursor_monitor`: always, unless the Mac sits under
/// one chosen monitor ([`under_monitor`]), which is then the only one that may.
pub fn bottom_allows(monitors: &[Area], cursor_monitor: &Area, placement: &Placement) -> bool {
    match under_monitor(monitors, placement) {
        Some(n) => monitor_number(monitors, cursor_monitor) == Some(n),
        None => true,
    }
}

/// Where the viewer window should be on `monitor`: filling it when full screen, otherwise its
/// current size (no bigger than the monitor) centred on it.
pub fn viewer_frame(monitor: Area, current: Area, full_screen: bool) -> Area {
    if full_screen {
        return monitor;
    }
    let (mw, mh) = (monitor.right - monitor.left, monitor.bottom - monitor.top);
    let (w, h) = (current.right - current.left, current.bottom - current.top);
    // A window that does not fit is shrunk, keeping its shape, to most of the monitor.
    let scale = 1f64.min(0.9 * mw / w).min(0.9 * mh / h);
    let (w, h) = ((w * scale).round(), (h * scale).round());
    let left = monitor.left + ((mw - w) / 2.0).round();
    let top = monitor.top + ((mh - h) / 2.0).round();
    Area { left, top, right: left + w, bottom: top + h }
}

/// Whether a window is mostly on this monitor already (its centre is on it).
pub fn is_on(window: Area, monitor: Area) -> bool {
    monitor.contains(((window.left + window.right) / 2.0, (window.top + window.bottom) / 2.0))
}

/// The monitor the viewer fills, when it is full screen.
pub fn filled_monitor(viewer: Area, monitors: &[Area]) -> Option<Area> {
    monitors.iter().copied().find(|m| *m == viewer)
}

/// Top-left corner for the top bar: centred along the top edge of the full-screen viewer.
pub fn bar_origin(viewer: Area, bar_size: (f64, f64)) -> (f64, f64) {
    (viewer.left + ((viewer.right - viewer.left - bar_size.0) / 2.0).round(), viewer.top)
}

/// The monitor after `current`, left to right, wrapping round.
pub fn next_monitor(monitors: &[Area], current: Area) -> Option<Area> {
    let mut sorted: Vec<Area> = monitors.to_vec();
    sorted.sort_by(|a, b| a.left.total_cmp(&b.left).then(a.top.total_cmp(&b.top)));
    let at = sorted.iter().position(|m| *m == current)?;
    let next = sorted[(at + 1) % sorted.len()];
    (next != current).then_some(next)
}

/// Height of the host window's title bar, in logical pixels.
pub const TITLE_BAR: i32 = 40;
/// Width of a freshly pulled-out display window, in logical pixels.
pub const PULL_WIDTH: i32 = 880;

/// The embedded Moonlight's rectangle inside the host window's client area: below the title bar
/// when windowed, the whole client when full screen.
pub fn child_rect(client_w: i32, client_h: i32, full_screen: bool, scale: f64) -> Area {
    let top = if full_screen { 0.0 } else { (TITLE_BAR as f64 * scale).round() };
    Area { left: 0.0, top, right: client_w as f64, bottom: client_h as f64 }
}

/// A window `PULL_WIDTH` wide (scaled) shaped like the stream, with its title bar centred under
/// the cursor, shifted to lie inside `work`.
pub fn pulled_window(cursor: (i32, i32), stream: (u32, u32), work: Area, scale: f64) -> Area {
    let bar = (TITLE_BAR as f64 * scale).round();
    let w = (PULL_WIDTH as f64 * scale).round();
    let h = (w * stream.1 as f64 / stream.0.max(1) as f64).round() + bar;
    let left = (cursor.0 as f64 - w / 2.0).min(work.right - w).max(work.left);
    let top = (cursor.1 as f64 - bar / 2.0).min(work.bottom - h).max(work.top);
    Area { left, top, right: left + w, bottom: top + h }
}

/// A window at `window` whose `visible` part (less any invisible resize frame) should lie inside
/// `work`: that part moves in at its size, or, bigger than `work`, shrinks to it; the window moves
/// and sizes with it. A window already inside is left exactly where it is.
pub fn clamp_into(window: Area, visible: Area, work: Area) -> Area {
    let w = (visible.right - visible.left).min(work.right - work.left);
    let h = (visible.bottom - visible.top).min(work.bottom - work.top);
    let left = visible.left.min(work.right - w).max(work.left);
    let top = visible.top.min(work.bottom - h).max(work.top);
    Area {
        left: window.left + (left - visible.left),
        top: window.top + (top - visible.top),
        right: window.right + (left + w - visible.right),
        bottom: window.bottom + (top + h - visible.bottom),
    }
}

/// How far left of the leftmost monitor a window is kept out of sight.
const OUT_OF_SIGHT: f64 = 12_000.0;

/// Where a window of `window`'s size is out of sight: left of every monitor (of the leftmost one,
/// so the scale Windows gives it is that monitor's), at the same height.
pub fn out_of_sight(window: Area, monitors: &[Area]) -> Area {
    let leftmost = monitors.iter().map(|m| m.left).fold(0.0, f64::min);
    let right = leftmost - OUT_OF_SIGHT;
    Area { left: right - (window.right - window.left), top: window.top, right, bottom: window.bottom }
}

/// What one of the viewer's windows is, by its class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewerKind {
    /// Its stream window (SDL's), the one that goes into the display window.
    Stream,
    /// A window it speaks to the user in: its launcher (where its errors and prompts are) or a
    /// message box.
    Talk,
    /// Anything else (Qt's and SDL's helpers, input method windows).
    Helper,
}

/// The class of Moonlight's stream window (an SDL window; seen live 2026-11-23).
pub const STREAM_CLASS: &str = "SDL_app";
/// The class of a Windows message box (SDL's own error boxes are these).
pub const MESSAGE_BOX_CLASS: &str = "#32770";

/// What kind of window of the viewer a class is.
pub fn viewer_kind(class: &str) -> ViewerKind {
    if class == STREAM_CLASS {
        ViewerKind::Stream
    } else if class == MESSAGE_BOX_CLASS || is_launcher_class(class) {
        ViewerKind::Talk
    } else {
        ViewerKind::Helper
    }
}

/// Whether a window class is a Qt program's own top-level window (Moonlight's launcher, where it
/// shows its errors): Qt names it `Qt<version>QWindow…Icon` (`QWindowIcon`, `QWindowOwnDCIcon`
/// for some GPU paths). Its tool windows, popups and tooltips end otherwise, and its helper
/// windows have other names.
pub fn is_launcher_class(class: &str) -> bool {
    class.starts_with("Qt") && class.contains("QWindow") && class.ends_with("Icon")
}

/// What the concealer knows of one of the viewer's windows as it hears of it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewerWindow {
    /// Still in its own frame: top-level, and not the window embedded in the display window.
    pub free: bool,
    pub kind: ViewerKind,
    pub visible: bool,
    pub on_a_monitor: bool,
}

/// What the concealer does with a window of the viewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewerAct {
    /// Out of sight: the stream window, from when it is made until it is embedded.
    Conceal,
    /// The viewer speaks to the user (a window of its own that shows, after a start that keeps
    /// its first window hidden): it is brought to the user at once, never hidden.
    Speaks,
    Leave,
}

/// What the concealer does with a window of the viewer it hears of: its stream window still in
/// its own frame and on a monitor goes out of sight; a window it speaks in that shows is the
/// viewer talking to the user (its first window, the launcher, was kept hidden by the start, so
/// any that shows now does so to say something: an error, a prompt); anything else is left alone.
pub fn viewer_act(w: ViewerWindow) -> ViewerAct {
    match w.kind {
        _ if !w.free => ViewerAct::Leave,
        ViewerKind::Stream if w.on_a_monitor => ViewerAct::Conceal,
        ViewerKind::Talk if w.visible => ViewerAct::Speaks,
        _ => ViewerAct::Leave,
    }
}

/// Whether a line of Moonlight's log (its stderr) is one of the errors that end its stream's start,
/// and its text. Only Moonlight's own known messages count, logged as errors (`Qt Critical`, or
/// `SDL Error` for the decoder's): `00:00:30 - Qt Critical: Failed to connect to 127.0.0.1` was seen
/// 2026-11-24 with Moonlight 6.2, and the others are its messages for the same place (strings in
/// Moonlight.exe 6.2). Anything else, even a critical line, is not: when unsure, the start goes
/// on (a message box Moonlight shows, and its window brought up after 15 s, still tell the user).
pub fn viewer_error(line: &str) -> Option<&str> {
    let (_, logged) = line.split_once(" - ")?;
    let text = match logged.strip_prefix("Qt Critical: ") {
        Some(text) => text,
        None => logged.strip_prefix("SDL Error (")?.split_once("): ")?.1,
    };
    let text = text.trim();
    is_start_error(text).then_some(text)
}

/// Moonlight's messages that end its stream's start.
fn is_start_error(text: &str) -> bool {
    const BEGINS: [&str; 5] = [
        "Failed to connect to ",
        "Failed to find application ",
        "Unable to connect to the specified PC",
        "Unable to initialize video decoder",
        "Failed to find ANY working H.264 or HEVC decoder",
    ];
    const HAS: [&str; 2] = ["has not been paired", "is not supported by this build of Moonlight"];
    // Its connection test's STUN server: not the host, and the start goes on.
    if text.starts_with("Failed to connect to STUN") {
        return false;
    }
    BEGINS.iter().any(|b| text.starts_with(b)) || HAS.iter().any(|h| text.contains(h))
}

/// Whether any part of `window` is on one of the `monitors` (touching an edge is not).
pub fn shows_on(window: Area, monitors: &[Area]) -> bool {
    monitors.iter().any(|m| {
        window.left < m.right && window.right > m.left && window.top < m.bottom && window.bottom > m.top
    })
}

/// "1 · 1920 × 1080": a monitor's number and size.
pub fn monitor_label(number: u32, a: &Area) -> String {
    format!("{number} \u{b7} {} \u{d7} {}", (a.right - a.left).round(), (a.bottom - a.top).round())
}

#[cfg(test)]
mod tests {
    use super::*;

    // The PC's two monitors, 2026-11-18 (physical pixels).
    const LEFT: Area = Area { left: -1920.0, top: 358.0, right: 0.0, bottom: 1438.0 };
    const RIGHT: Area = Area { left: 0.0, top: 0.0, right: 2560.0, bottom: 1440.0 };

    #[test]
    fn with_the_mac_on_the_right_the_viewer_goes_on_the_right_monitor() {
        assert_eq!(monitor_toward(&[LEFT, RIGHT], Side::Right), Some(RIGHT));
        assert_eq!(monitor_toward(&[RIGHT, LEFT], Side::Right), Some(RIGHT));
    }

    #[test]
    fn with_the_mac_on_the_left_the_viewer_goes_on_the_left_monitor() {
        assert_eq!(monitor_toward(&[LEFT, RIGHT], Side::Left), Some(LEFT));
    }

    #[test]
    fn no_monitors_gives_nothing() {
        assert_eq!(monitor_toward(&[], Side::Right), None);
    }

    #[test]
    fn full_screen_fills_the_monitor() {
        let current = Area { left: -1701.0, top: 431.0, right: -220.0, bottom: 1294.0 };
        assert_eq!(viewer_frame(RIGHT, current, true), RIGHT);
    }

    #[test]
    fn a_window_keeps_its_size_and_is_centred() {
        let current = Area { left: -1701.0, top: 431.0, right: -221.0, bottom: 1291.0 };
        assert_eq!(viewer_frame(RIGHT, current, false), Area { left: 540.0, top: 290.0, right: 2020.0, bottom: 1150.0 });
    }

    #[test]
    fn a_window_bigger_than_the_monitor_is_shrunk_to_fit() {
        let current = Area { left: 0.0, top: 0.0, right: 3000.0, bottom: 2000.0 };
        let f = viewer_frame(LEFT, current, false);
        assert!(f.right - f.left <= 1920.0 && f.bottom - f.top <= 1080.0, "{f:?}");
        assert!(is_on(f, LEFT));
    }

    #[test]
    fn a_window_centred_on_a_monitor_is_on_it() {
        assert!(is_on(Area { left: 540.0, top: 290.0, right: 2020.0, bottom: 1150.0 }, RIGHT));
        assert!(!is_on(Area { left: -1701.0, top: 431.0, right: -220.0, bottom: 1294.0 }, RIGHT));
    }

    #[test]
    fn a_viewer_covering_a_monitor_exactly_is_full_screen() {
        assert_eq!(filled_monitor(RIGHT, &[LEFT, RIGHT]), Some(RIGHT));
        let window = Area { left: 540.0, top: 290.0, right: 2020.0, bottom: 1150.0 };
        assert_eq!(filled_monitor(window, &[LEFT, RIGHT]), None);
    }

    #[test]
    fn the_bar_sits_centred_on_the_viewers_top_edge() {
        assert_eq!(bar_origin(RIGHT, (160.0, 10.0)), (1200.0, 0.0));
        assert_eq!(bar_origin(LEFT, (200.0, 40.0)), (-1060.0, 358.0));
    }

    #[test]
    fn monitors_are_numbered_left_to_right_then_top_to_bottom() {
        let monitors = [RIGHT, LEFT];
        assert_eq!(monitor_number(&monitors, &LEFT), Some(1));
        assert_eq!(monitor_number(&monitors, &RIGHT), Some(2));
        assert_eq!(monitor_by_number(&monitors, 1), Some(LEFT));
        assert_eq!(monitor_by_number(&monitors, 2), Some(RIGHT));
        assert_eq!(monitor_by_number(&monitors, 0), None);
        assert_eq!(monitor_by_number(&monitors, 3), None);
        // Same left edge: the higher one is first.
        let low = Area { left: 0.0, top: 1440.0, right: 1920.0, bottom: 2520.0 };
        assert_eq!(monitor_number(&[low, RIGHT], &RIGHT), Some(1));
        assert_eq!(monitor_number(&[low, RIGHT], &low), Some(2));
        let elsewhere = Area { left: 9.0, top: 9.0, right: 10.0, bottom: 10.0 };
        assert_eq!(monitor_number(&monitors, &elsewhere), None);
    }

    #[test]
    fn a_mac_under_one_monitor_takes_the_pointer_only_from_it() {
        let ms = [RIGHT, LEFT];
        let under = |n| Placement { side: Side::Bottom, monitor: Some(n) };
        assert!(bottom_allows(&ms, &LEFT, &under(1)));
        assert!(!bottom_allows(&ms, &RIGHT, &under(1)));
        assert!(bottom_allows(&ms, &RIGHT, &under(2)));
        let any = Placement { side: Side::Bottom, monitor: None };
        assert!(bottom_allows(&ms, &LEFT, &any) && bottom_allows(&ms, &RIGHT, &any));
        let right = Placement { side: Side::Right, monitor: Some(1) };
        assert!(bottom_allows(&ms, &RIGHT, &right));
    }

    #[test]
    fn the_viewer_opens_on_the_chosen_monitor() {
        let placement = Placement { side: Side::Right, monitor: Some(1) };
        assert_eq!(viewer_monitor(&[LEFT, RIGHT], Some(1), &placement), Some(LEFT));
        assert_eq!(viewer_monitor(&[LEFT, RIGHT], Some(2), &placement), Some(RIGHT));
    }

    #[test]
    fn under_a_monitor_the_viewer_defaults_to_that_monitor() {
        let placement = Placement { side: Side::Bottom, monitor: Some(1) };
        assert_eq!(viewer_monitor(&[LEFT, RIGHT], None, &placement), Some(LEFT));
        assert_eq!(viewer_monitor(&[LEFT, RIGHT], Some(2), &placement), Some(RIGHT), "a chosen stream monitor wins");
    }

    // Plan 2 final review, item 3: the window drew a Mac "under monitor 5" under monitor 1 once
    // monitor 5 was unplugged, while crossing refused every monitor. Both use one rule now.
    #[test]
    fn under_a_missing_monitor_counts_as_under_the_lowest_numbered_one() {
        let ms = [RIGHT, LEFT];
        let gone = Placement { side: Side::Bottom, monitor: Some(5) };
        assert_eq!(under_monitor(&ms, &gone), Some(1));
        assert!(bottom_allows(&ms, &LEFT, &gone), "monitor 1 takes the pointer down to the Mac");
        assert!(!bottom_allows(&ms, &RIGHT, &gone));
        assert_eq!(viewer_monitor(&ms, None, &gone), Some(LEFT));
        assert_eq!(viewer_monitor(&ms, Some(2), &gone), Some(RIGHT), "a chosen stream monitor still wins");
        assert_eq!(under_monitor(&ms, &Placement { side: Side::Bottom, monitor: Some(2) }), Some(2));
        assert_eq!(under_monitor(&ms, &Placement { side: Side::Right, monitor: Some(2) }), None);
        assert_eq!(under_monitor(&[], &gone), None);
    }

    #[test]
    fn a_missing_chosen_monitor_falls_back_toward_the_mac() {
        let placement = Placement { side: Side::Left, monitor: None };
        assert_eq!(viewer_monitor(&[LEFT, RIGHT], Some(5), &placement), Some(LEFT));
        assert_eq!(viewer_monitor(&[LEFT, RIGHT], None, &placement), Some(LEFT));
        assert_eq!(viewer_monitor(&[], Some(1), &placement), None);
    }

    #[test]
    fn the_next_monitor_goes_left_to_right_and_wraps() {
        assert_eq!(next_monitor(&[RIGHT, LEFT], LEFT), Some(RIGHT));
        assert_eq!(next_monitor(&[RIGHT, LEFT], RIGHT), Some(LEFT));
        assert_eq!(next_monitor(&[RIGHT], RIGHT), None);
    }

    #[test]
    fn numbered_monitors_match_monitor_number() {
        let low = Area { left: 0.0, top: 900.0, right: 100.0, bottom: 1000.0 };
        let monitors = [RIGHT, low, LEFT];
        let numbered = numbered_monitors(&monitors);
        assert_eq!(numbered.iter().map(|(n, _)| *n).collect::<Vec<_>>(), vec![1, 2, 3]);
        for (n, m) in numbered {
            assert_eq!(monitor_number(&monitors, &m), Some(n));
            assert_eq!(monitor_by_number(&monitors, n), Some(m));
        }
    }

    #[test]
    fn the_child_sits_below_the_title_bar_unless_full_screen() {
        assert_eq!(child_rect(1280, 760, false, 1.0), Area { left: 0.0, top: 40.0, right: 1280.0, bottom: 760.0 });
        assert_eq!(child_rect(1280, 760, true, 1.0), Area { left: 0.0, top: 0.0, right: 1280.0, bottom: 760.0 });
        assert_eq!(child_rect(1920, 1140, false, 1.5).top, 60.0);
    }

    #[test]
    fn a_pulled_window_is_centred_under_the_cursor_and_clamped() {
        let work = Area { left: 0.0, top: 0.0, right: 2560.0, bottom: 1400.0 };
        let w = pulled_window((1000, 10), (2560, 1440), work, 1.0);
        assert_eq!((w.left, w.top, w.right, w.bottom), (560.0, 0.0, 1440.0, 535.0));
        let edge = pulled_window((2550, 700), (2560, 1440), work, 1.0);
        assert_eq!(edge.right, 2560.0);
        assert_eq!(edge.right - edge.left, 880.0);
    }

    #[test]
    fn monitor_labels_read_number_and_size() {
        assert_eq!(monitor_label(2, &Area { left: 0.0, top: 0.0, right: 2560.0, bottom: 1440.0 }), "2 \u{b7} 2560 \u{d7} 1440");
    }

    // Monitor 1 of the PC (100%, left of the main one), less a 48 px taskbar.
    const WORK: Area = Area { left: -1920.0, top: 358.0, right: 0.0, bottom: 1390.0 };
    /// Tauri's frameless window keeps an invisible resize frame left, right and below.
    fn framed(v: Area) -> Area {
        Area { left: v.left - 7.0, top: v.top, right: v.right + 7.0, bottom: v.bottom + 7.0 }
    }

    #[test]
    fn a_window_inside_its_work_area_stays() {
        let v = Area { left: -1800.0, top: 400.0, right: -600.0, bottom: 1100.0 };
        assert_eq!(clamp_into(framed(v), v, WORK), framed(v));
    }

    #[test]
    fn a_window_snapped_to_the_work_areas_edges_stays() {
        // Its invisible frame hangs over the edges; what shows is exactly the work area.
        assert_eq!(clamp_into(framed(WORK), WORK, WORK), framed(WORK));
    }

    #[test]
    fn a_window_hanging_below_comes_up_at_its_size() {
        // Dragged here from the 125% monitor: sized for that, it ends below the taskbar.
        let v = Area { left: -1700.0, top: 600.0, right: -100.0, bottom: 1550.0 };
        let in_work = Area { left: -1700.0, top: 440.0, right: -100.0, bottom: 1390.0 };
        assert_eq!(clamp_into(framed(v), v, WORK), framed(in_work));
    }

    #[test]
    fn a_window_hanging_off_a_side_comes_back_at_its_size() {
        let v = Area { left: -2100.0, top: 300.0, right: -900.0, bottom: 1000.0 };
        let in_work = Area { left: -1920.0, top: 358.0, right: -720.0, bottom: 1058.0 };
        assert_eq!(clamp_into(framed(v), v, WORK), framed(in_work));
    }

    #[test]
    fn a_window_bigger_than_the_work_area_shrinks_to_fit() {
        let v = Area { left: -1900.0, top: 380.0, right: 300.0, bottom: 1600.0 };
        assert_eq!(clamp_into(framed(v), v, WORK), framed(WORK));
    }

    // The PC's monitors: 1 left of 2, at other heights.
    const MONITORS: [Area; 2] = [LEFT, RIGHT];

    #[test]
    fn a_window_put_out_of_sight_is_off_every_monitor_at_its_size() {
        let w = Area { left: 400.0, top: 200.0, right: 1680.0, bottom: 960.0 };
        let away = out_of_sight(w, &MONITORS);
        assert_eq!((away.right - away.left, away.bottom - away.top), (1280.0, 760.0));
        assert!(!shows_on(away, &MONITORS));
        // Left of the leftmost monitor, by a margin.
        assert!(away.right < LEFT.left);
    }

    #[test]
    fn with_no_monitors_a_window_still_goes_far_away() {
        let w = Area { left: 0.0, top: 0.0, right: 100.0, bottom: 100.0 };
        assert!(out_of_sight(w, &[]).right < -10_000.0);
    }

    fn act(free: bool, kind: ViewerKind, visible: bool, on: bool) -> ViewerAct {
        viewer_act(ViewerWindow { free, kind, visible, on_a_monitor: on })
    }

    #[test]
    fn windows_are_known_by_their_class() {
        assert_eq!(viewer_kind("SDL_app"), ViewerKind::Stream);
        assert_eq!(viewer_kind("Qt6120QWindowIcon"), ViewerKind::Talk);
        assert_eq!(viewer_kind("Qt6120QWindowOwnDCIcon"), ViewerKind::Talk);
        assert_eq!(viewer_kind("#32770"), ViewerKind::Talk);
        assert_eq!(viewer_kind("Qt6120QWindowToolSaveBits"), ViewerKind::Helper);
        assert_eq!(viewer_kind("Qt6120QWindowPopupDropShadowSaveBits"), ViewerKind::Helper);
        assert_eq!(viewer_kind("Qt6120QWindowToolTipDropShadowSaveBits"), ViewerKind::Helper);
        assert_eq!(viewer_kind("Qt6120PowerDummyWindow"), ViewerKind::Helper);
        assert_eq!(viewer_kind("Qt6120ThemeChangeObserverWindow"), ViewerKind::Helper);
        assert_eq!(viewer_kind("QEventDispatcherWin32_Internal_Widget140704566533760"), ViewerKind::Helper);
        assert_eq!(viewer_kind("SDLHelperWindowInputCatcher"), ViewerKind::Helper);
        assert_eq!(viewer_kind("IME"), ViewerKind::Helper);
    }

    #[test]
    fn the_stream_window_goes_out_of_sight_from_when_it_is_made() {
        assert_eq!(act(true, ViewerKind::Stream, false, true), ViewerAct::Conceal);
        assert_eq!(act(true, ViewerKind::Stream, true, true), ViewerAct::Conceal);
    }

    #[test]
    fn a_window_the_viewer_speaks_in_that_shows_is_brought_to_the_user() {
        // Its launcher shown again (an error after the stream ended), a message box.
        assert_eq!(act(true, ViewerKind::Talk, true, true), ViewerAct::Speaks);
        // Shown off every monitor still speaks: it is brought to the user.
        assert_eq!(act(true, ViewerKind::Talk, true, false), ViewerAct::Speaks);
        // Made, not shown: nothing said yet.
        assert_eq!(act(true, ViewerKind::Talk, false, true), ViewerAct::Leave);
    }

    #[test]
    fn helpers_embedded_and_out_of_sight_windows_are_left_alone() {
        assert_eq!(act(true, ViewerKind::Helper, true, true), ViewerAct::Leave);
        assert_eq!(act(false, ViewerKind::Stream, true, true), ViewerAct::Leave);
        assert_eq!(act(true, ViewerKind::Stream, true, false), ViewerAct::Leave);
    }

    #[test]
    fn an_error_moonlight_puts_up_is_read_from_its_log() {
        assert_eq!(viewer_error("00:00:30 - Qt Critical: Failed to connect to 127.0.0.1"), Some("Failed to connect to 127.0.0.1"));
        assert_eq!(viewer_error("00:00:04 - Qt Warning: \"serverinfo\" request failed with error: QNetworkReply::ConnectionRefusedError"), None);
        assert_eq!(viewer_error("00:00:00 - Qt Info: Processing new PC at \"127.0.0.1:47989\" from user"), None);
        assert_eq!(viewer_error("00:00:30 - Qt Critical:   "), None);
        assert_eq!(viewer_error(""), None);
    }

    #[test]
    fn moonlights_known_errors_end_a_start() {
        // Seen live (Moonlight 6.2, a host that refuses).
        assert_eq!(viewer_error("00:00:30 - Qt Critical: Failed to connect to 192.168.1.50"), Some("Failed to connect to 192.168.1.50"));
        assert_eq!(viewer_error("00:00:30 - Qt Critical: Failed to connect to fe80::1%12"), Some("Failed to connect to fe80::1%12"));
        // Its other messages for the stream's start (strings in Moonlight.exe 6.2).
        let paired = "00:00:01 - Qt Critical: Computer Mac.lan has not been paired. Please open Moonlight to pair before streaming.";
        assert!(viewer_error(paired).is_some());
        assert_eq!(viewer_error("00:00:01 - Qt Critical: Failed to find application Desktop"), Some("Failed to find application Desktop"));
        assert!(viewer_error("00:00:01 - Qt Critical: The version of GeForce Experience on Mac is not supported by this build of Moonlight. You must update Moonlight to stream from Mac.").is_some());
        assert!(viewer_error("00:00:02 - Qt Critical: Unable to connect to the specified PC.").is_some());
        assert!(viewer_error("00:00:03 - SDL Error (0): Unable to initialize video decoder. Please check your streaming settings and try again.").is_some());
        assert!(viewer_error("00:00:03 - SDL Error (0): Failed to find ANY working H.264 or HEVC decoder!").is_some());
    }

    #[test]
    fn other_critical_lines_and_warnings_never_end_a_start() {
        for line in [
            // Benign or recoverable lines at the levels the known errors use.
            "00:00:00 - Qt Critical: QWindowsWindow::setGeometry: Unable to set geometry 1294x638+377+222 on QQuickWindowQmlImpl",
            "00:00:01 - Qt Critical: Failed to connect to STUN server: 10060",
            "00:00:02 - SDL Error (0): RTSP: Failed to connect to UDP port 48000: error 10061",
            "00:00:02 - SDL Error (0): Unable to open decoder for format: 1",
            "00:00:02 - SDL Error (0): Unable to set render thread to high priority: Access is denied.",
            "00:00:02 - Qt Critical: Unable to load gamepad mapping file",
            // A known message, but only a warning or information.
            "00:00:30 - Qt Warning: Failed to connect to 192.168.1.50",
            "00:00:30 - Qt Info: Failed to connect to 192.168.1.50",
            "00:00:04 - Qt Warning: \"serverinfo\" request failed with error: QNetworkReply::ContentNotFoundError",
            "00:00:45 - Qt Warning: QDxgiVSyncService not destroyed in time",
            "Failed to connect to 192.168.1.50",
        ] {
            assert_eq!(viewer_error(line), None, "{line}");
        }
    }

    #[test]
    fn a_window_shows_on_a_monitor_it_overlaps() {
        assert!(shows_on(Area { left: -100.0, top: 400.0, right: 100.0, bottom: 600.0 }, &MONITORS));
        assert!(shows_on(Area { left: 2500.0, top: 1400.0, right: 2700.0, bottom: 1500.0 }, &MONITORS));
    }

    #[test]
    fn a_window_beside_every_monitor_does_not_show() {
        // Touching an edge is not showing.
        assert!(!shows_on(Area { left: 2560.0, top: 0.0, right: 2700.0, bottom: 100.0 }, &MONITORS));
        // Above monitor 1, which starts lower than monitor 2.
        assert!(!shows_on(Area { left: -1900.0, top: 0.0, right: -100.0, bottom: 358.0 }, &MONITORS));
    }
}
