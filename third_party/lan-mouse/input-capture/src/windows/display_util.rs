use windows::Win32::Foundation::RECT;

use crate::Position;

fn is_within_dp_region(point: (i32, i32), display: &RECT) -> bool {
    [
        Position::Left,
        Position::Right,
        Position::Top,
        Position::Bottom,
    ]
    .iter()
    .all(|&pos| is_within_dp_boundary(point, display, pos))
}

fn is_within_dp_boundary(point: (i32, i32), display: &RECT, pos: Position) -> bool {
    let (x, y) = point;
    match pos {
        Position::Left => display.left <= x,
        Position::Right => display.right > x,
        Position::Top => display.top <= y,
        Position::Bottom => display.bottom > y,
    }
}

/// returns whether the given position is within the display bounds with respect to the given
/// barrier position
///
/// # Arguments
///
/// * `x`:
/// * `y`:
/// * `displays`:
/// * `pos`:
///
/// returns: bool
///
fn in_bounds(point: (i32, i32), displays: &[RECT], pos: Position) -> bool {
    displays
        .iter()
        .any(|d| is_within_dp_boundary(point, d, pos))
}

fn in_display_region(point: (i32, i32), displays: &[RECT]) -> bool {
    displays.iter().any(|d| is_within_dp_region(point, d))
}

fn moved_across_boundary(
    prev_pos: (i32, i32),
    curr_pos: (i32, i32),
    displays: &[RECT],
    pos: Position,
) -> bool {
    /* was within bounds, but is not anymore */
    in_display_region(prev_pos, displays) && !in_bounds(curr_pos, displays, pos)
}

pub(crate) fn entered_barrier(
    prev_pos: (i32, i32),
    curr_pos: (i32, i32),
    displays: &[RECT],
) -> Option<Position> {
    [
        Position::Left,
        Position::Right,
        Position::Top,
        Position::Bottom,
    ]
    .into_iter()
    .find(|&pos| moved_across_boundary(prev_pos, curr_pos, displays, pos))
}

///
/// clamp point to display bounds
///
/// # Arguments
///
/// * `prev_point`: coordinates, the cursor was before entering, within bounds of a display
/// * `entry_point`: point to clamp
///
/// returns: (i32, i32), the corrected entry point
///
pub(crate) fn clamp_to_display_bounds(
    display_regions: &[RECT],
    prev_point: (i32, i32),
    point: (i32, i32),
) -> (i32, i32) {
    /* find display where movement came from */
    let display = display_regions
        .iter()
        .find(|&d| is_within_dp_region(prev_point, d))
        .unwrap();

    /* clamp to bounds (inclusive) */
    let (x, y) = point;
    let (min_x, max_x) = (display.left, display.right - 1);
    let (min_y, max_y) = (display.top, display.bottom - 1);
    (x.clamp(min_x, max_x), y.clamp(min_y, max_y))
}

/// Sidescreen: the position to compare the next pointer move against. Only positions on a
/// screen are remembered: after a capture is released with the pointer pushed against the
/// edge, the reported positions lie beyond it, and remembering those meant pushing on never
/// crossed again until the pointer was moved back first.
pub(crate) fn remembered_position(
    prev: Option<(i32, i32)>,
    curr: (i32, i32),
    displays: &[RECT],
) -> Option<(i32, i32)> {
    if in_display_region(curr, displays) {
        Some(curr)
    } else {
        prev
    }
}

#[cfg(test)]
mod sidescreen_tests {
    use super::*;

    // The PC's right-hand monitor, 2026-11-18 (physical pixels).
    fn displays() -> Vec<RECT> {
        vec![RECT { left: 0, top: 0, right: 2560, bottom: 1440 }]
    }

    #[test]
    fn a_position_on_a_screen_is_remembered() {
        assert_eq!(remembered_position(Some((2559, 700)), (2500, 700), &displays()), Some((2500, 700)));
    }

    #[test]
    fn a_position_beyond_the_edge_is_not_remembered() {
        assert_eq!(remembered_position(Some((2559, 700)), (2579, 700), &displays()), Some((2559, 700)));
    }

    // The bug: pushing on against the edge after a released capture never crossed again.
    #[test]
    fn pushing_on_against_the_edge_keeps_crossing() {
        let d = displays();
        let mut prev = Some((2559, 700));
        for _ in 0..3 {
            let curr = (2579, 700);
            assert_eq!(entered_barrier(prev.unwrap(), curr, &d), Some(Position::Right));
            prev = remembered_position(prev, curr, &d);
        }
    }
}
