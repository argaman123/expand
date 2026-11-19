//! Where the pointer goes when control passes between the PC and the Mac. Pure functions, so
//! every rule here is tested without either machine.

use crate::side::Side;

/// A rectangle in a screen coordinate space (Mac points or PC pixels).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Area {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
}

impl Area {
    pub fn contains(&self, (x, y): (f64, f64)) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }
}

/// How far inside an edge the pointer lands: on the edge itself, the next move that is not
/// inward would count as leaving again.
pub const MARGIN: f64 = 8.0;

/// What the Mac tells the PC when it hands control back.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct HandBack {
    /// Where the pointer is on the virtual display (0..1 across, 0..1 down), if it is there.
    pub on_virtual: Option<(f64, f64)>,
    /// How far along the edge facing the PC the pointer left (0..1).
    pub along: f64,
}

/// 0..1 position of `point` along the edge of `area` on `side`.
pub fn along_edge(area: Area, point: (f64, f64), side: Side) -> f64 {
    let fraction = match side {
        Side::Left | Side::Right => (point.1 - area.top) / (area.bottom - area.top),
        Side::Top | Side::Bottom => (point.0 - area.left) / (area.right - area.left),
    };
    fraction.clamp(0.0, 1.0)
}

/// Where the pointer lands on the MacBook's screen when the PC hands over: just inside the edge
/// facing the PC (`pc_side`), `along` that edge.
pub fn landing_on_mac(main: Area, pc_side: Side, along: f64) -> (f64, f64) {
    let x = main.left + along * (main.right - main.left);
    let y = main.top + along * (main.bottom - main.top);
    match pc_side {
        Side::Left => (main.left + MARGIN, y),
        Side::Right => (main.right - MARGIN, y),
        Side::Top => (x, main.top + MARGIN),
        Side::Bottom => (x, main.bottom - MARGIN),
    }
}

/// Moves the Mac pointer by a relative step. A step that would leave every display stops at
/// the edge of the display the pointer is on, as a physical mouse would.
pub fn step(displays: &[Area], from: (f64, f64), delta: (f64, f64)) -> (f64, f64) {
    let to = (from.0 + delta.0, from.1 + delta.1);
    if displays.iter().any(|d| d.contains(to)) {
        return to;
    }
    match displays.iter().find(|d| d.contains(from)).or(displays.first()) {
        Some(d) => (to.0.clamp(d.left, d.right - 1.0), to.1.clamp(d.top, d.bottom - 1.0)),
        None => from,
    }
}

/// Decides, after the PC's pointer moved the Mac's (or released a button), whether control goes
/// back to the PC. `attempted` is where the move would have gone before stopping at an edge.
pub fn hand_back(
    main: Area,
    virtual_display: Option<Area>,
    pc_side: Side,
    pointer: (f64, f64),
    attempted: (f64, f64),
    button_held: bool,
) -> Option<HandBack> {
    if button_held {
        return None;
    }
    if let Some(v) = virtual_display.filter(|v| v.contains(pointer)) {
        let across = (pointer.0 - v.left) / (v.right - v.left);
        let down = (pointer.1 - v.top) / (v.bottom - v.top);
        return Some(HandBack { on_virtual: Some((across, down)), along: along_edge(main, pointer, pc_side) });
    }
    let past_pc_edge = match pc_side {
        Side::Left => attempted.0 < main.left,
        Side::Right => attempted.0 >= main.right,
        Side::Top => attempted.1 < main.top,
        Side::Bottom => attempted.1 >= main.bottom,
    };
    (main.contains(pointer) && past_pc_edge)
        .then(|| HandBack { on_virtual: None, along: along_edge(main, pointer, pc_side) })
}

/// Where the PC's pointer lands when the Mac hands back. On the viewer at the matching spot when
/// the Mac pointer is on the virtual display and the viewer's picture area is known; otherwise
/// just inside the Mac-side edge of `monitor` (the one the pointer left from), `along` it.
pub fn landing_on_pc(monitor: Area, mac_side: Side, back: &HandBack, viewer_picture: Option<Area>) -> (f64, f64) {
    if let (Some((across, down)), Some(p)) = (back.on_virtual, viewer_picture) {
        return (p.left + across * (p.right - p.left), p.top + down * (p.bottom - p.top));
    }
    let x = monitor.left + back.along * (monitor.right - monitor.left);
    let y = monitor.top + back.along * (monitor.bottom - monitor.top);
    match mac_side {
        Side::Left => (monitor.left + MARGIN, y),
        Side::Right => (monitor.right - MARGIN, y),
        Side::Top => (x, monitor.top + MARGIN),
        Side::Bottom => (x, monitor.bottom - MARGIN),
    }
}

/// The part of the viewer's window that shows the picture: the largest area of the stream's
/// shape centred in the window (Moonlight adds bars when the shapes differ).
pub fn picture_area(window: Area, stream: (u32, u32)) -> Area {
    let (w, h) = (window.right - window.left, window.bottom - window.top);
    let (sw, sh) = (f64::from(stream.0.max(1)), f64::from(stream.1.max(1)));
    let scale = (w / sw).min(h / sh);
    let (pw, ph) = (sw * scale, sh * scale);
    let left = window.left + (w - pw) / 2.0;
    let top = window.top + (h - ph) / 2.0;
    Area { left, top, right: left + pw, bottom: top + ph }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Measured on 2026-11-18: the MacBook's screen, the virtual display beside it on the PC's
    // side (points), and the PC's right-hand monitor (pixels).
    const MAIN: Area = Area { left: 0.0, top: 0.0, right: 1728.0, bottom: 1117.0 };
    const VIRTUAL: Area = Area { left: -2560.0, top: -161.0, right: 0.0, bottom: 1279.0 };
    const PC_RIGHT: Area = Area { left: 0.0, top: 0.0, right: 2560.0, bottom: 1440.0 };

    #[test]
    fn along_an_edge_is_measured_from_its_top_or_left() {
        assert_eq!(along_edge(PC_RIGHT, (2559.0, 720.0), Side::Right), 0.5);
        assert_eq!(along_edge(PC_RIGHT, (640.0, 0.0), Side::Top), 0.25);
    }

    #[test]
    fn the_pointer_lands_inside_the_macbooks_edge_at_the_same_height() {
        assert_eq!(landing_on_mac(MAIN, Side::Left, 0.5), (8.0, 558.5));
        assert_eq!(landing_on_mac(MAIN, Side::Right, 0.0), (1720.0, 0.0));
    }

    #[test]
    fn a_step_onto_another_display_crosses_over() {
        assert_eq!(step(&[MAIN, VIRTUAL], (5.0, 300.0), (-20.0, 0.0)), (-15.0, 300.0));
    }

    #[test]
    fn a_step_off_every_display_stops_at_the_edge() {
        assert_eq!(step(&[MAIN, VIRTUAL], (1700.0, 300.0), (50.0, 0.0)), (1727.0, 300.0));
        assert_eq!(step(&[MAIN], (5.0, 300.0), (-20.0, 0.0)), (0.0, 300.0));
    }

    #[test]
    fn moving_on_the_macbook_keeps_control() {
        assert_eq!(hand_back(MAIN, Some(VIRTUAL), Side::Left, (500.0, 300.0), (500.0, 300.0), false), None);
    }

    #[test]
    fn moving_onto_the_virtual_display_hands_back_with_the_spot() {
        let back = hand_back(MAIN, Some(VIRTUAL), Side::Left, (-256.0, 559.0), (-256.0, 559.0), false).unwrap();
        assert_eq!(back.on_virtual, Some((0.9, 0.5)));
    }

    // Dragging a window over to the virtual display keeps control until the button is let go.
    #[test]
    fn dragging_onto_the_virtual_display_keeps_control() {
        assert_eq!(hand_back(MAIN, Some(VIRTUAL), Side::Left, (-256.0, 559.0), (-256.0, 559.0), true), None);
    }

    #[test]
    fn without_a_virtual_display_pushing_past_the_edge_facing_the_pc_hands_back() {
        let back = hand_back(MAIN, None, Side::Left, (0.0, 558.5), (-12.0, 558.5), false).unwrap();
        assert_eq!(back, HandBack { on_virtual: None, along: 0.5 });
    }

    #[test]
    fn pushing_past_another_edge_keeps_control() {
        assert_eq!(hand_back(MAIN, None, Side::Left, (1727.0, 300.0), (1760.0, 300.0), false), None);
    }

    #[test]
    fn back_on_the_pc_the_pointer_goes_to_the_matching_spot_in_the_viewer() {
        let picture = Area { left: 0.0, top: 0.0, right: 2560.0, bottom: 1440.0 };
        let back = HandBack { on_virtual: Some((0.9, 0.5)), along: 0.5 };
        assert_eq!(landing_on_pc(PC_RIGHT, Side::Right, &back, Some(picture)), (2304.0, 720.0));
    }

    #[test]
    fn back_on_the_pc_without_the_viewer_the_pointer_comes_in_at_the_edge() {
        let back = HandBack { on_virtual: Some((0.9, 0.5)), along: 0.25 };
        assert_eq!(landing_on_pc(PC_RIGHT, Side::Right, &back, None), (2552.0, 360.0));
    }

    #[test]
    fn a_wider_window_shows_the_picture_with_bars_at_the_sides() {
        let window = Area { left: 0.0, top: 0.0, right: 2000.0, bottom: 900.0 };
        assert_eq!(picture_area(window, (1600, 900)), Area { left: 200.0, top: 0.0, right: 1800.0, bottom: 900.0 });
    }

    #[test]
    fn a_taller_window_shows_the_picture_with_bars_above_and_below() {
        let window = Area { left: 100.0, top: 0.0, right: 1700.0, bottom: 1000.0 };
        assert_eq!(picture_area(window, (1600, 900)), Area { left: 100.0, top: 50.0, right: 1700.0, bottom: 950.0 });
    }
}
