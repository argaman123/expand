//! Where the viewer window goes on the PC: on the monitor next to the Mac. Pure functions; the
//! Windows calls live in `winwindow`.

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
    fn the_next_monitor_goes_left_to_right_and_wraps() {
        assert_eq!(next_monitor(&[RIGHT, LEFT], LEFT), Some(RIGHT));
        assert_eq!(next_monitor(&[RIGHT, LEFT], RIGHT), Some(LEFT));
        assert_eq!(next_monitor(&[RIGHT], RIGHT), None);
    }
}
