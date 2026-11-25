//! Loaded by Windows into the viewer (Moonlight) for an in-context window-event hook
//! (`SetWinEventHook` with `WINEVENT_INCONTEXT`, set by `expand_core::winwindow::Concealer`):
//! [`expand_viewer_event`] runs on the viewer's own thread, inside the call that makes, shows
//! or moves its window, so its stream window is moved out of sight before Windows ever draws it
//! on a monitor. Out of context, the move had to wait for the viewer's thread, which is busy
//! right then (seen live: 70 ms on screen).
//!
//! It only reads its own window and moves it: nothing is kept, nothing else is touched.

/// The class of the viewer's stream window (an SDL window).
pub const STREAM_CLASS: &str = "SDL_app";

/// How far left of every monitor the window goes when there is no place beside its own monitor
/// (as `expand_core::layout::out_of_sight`).
pub const OUT_OF_SIGHT: i32 = 12_000;

/// How far from its monitor the window waits, at least: more than its shadow reaches, so not a
/// pixel of it shows, and no other monitor is nearer.
pub const GAP: i32 = 100;

/// A rectangle in screen pixels: left, top, right, bottom.
pub type Rect = (i32, i32, i32, i32);

fn overlap(a: Rect, b: Rect) -> i64 {
    let w = (a.2.min(b.2) - a.0.max(b.0)).max(0) as i64;
    let h = (a.3.min(b.3) - a.1.max(b.1)).max(0) as i64;
    w * h
}

fn grown(r: Rect, by: i32) -> Rect {
    (r.0 - by, r.1 - by, r.2 + by, r.3 + by)
}

/// Where a window at `window` waits on no monitor (its top-left corner), with `monitors` the
/// monitors' rects; `None` when it is on none already (this hook put it there, or the viewer
/// did). Just beside the monitor it is on (the one it overlaps most): above it, else below, left
/// or right, [`GAP`] away and with no other monitor as near, so Windows still counts it on that
/// monitor (its scale, its refresh rate: the viewer sets itself up for the monitor it chose, as
/// without the hook). With no such place, left of every monitor.
pub fn hiding_place(window: Rect, monitors: &[Rect]) -> Option<(i32, i32)> {
    let mine = monitors.iter().copied().filter(|m| overlap(window, *m) > 0).max_by_key(|m| overlap(window, *m))?;
    let (w, h) = (window.2 - window.0, window.3 - window.1);
    let (x, y) = (window.0, window.1);
    let (ml, mt, mr, mb) = mine;
    let beside = [
        (x, mt - GAP - h),
        (x, mb + GAP),
        (ml - GAP - w, y),
        (mr + GAP, y),
    ];
    let clear = |(cx, cy): (i32, i32)| {
        let spot = grown((cx, cy, cx + w, cy + h), GAP - 1);
        monitors.iter().all(|m| overlap(spot, *m) == 0)
    };
    Some(beside.into_iter().find(|c| clear(*c)).unwrap_or_else(|| {
        let leftmost = monitors.iter().map(|m| m.0).min().unwrap_or(0);
        (leftmost - OUT_OF_SIGHT - w, y)
    }))
}

#[cfg(windows)]
mod hook {
    use super::{hiding_place, Rect, STREAM_CLASS};
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
    use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, HDC, HMONITOR};
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::Accessibility::HWINEVENTHOOK;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetClassNameW, GetWindowLongPtrW, GetWindowRect, GetWindowThreadProcessId, SetWindowPos, CHILDID_SELF,
        GWL_STYLE, OBJID_WINDOW, SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, WS_CHILD,
    };

    /// The monitors' rects. Reads only (no message is sent).
    pub(crate) fn monitors() -> Vec<Rect> {
        unsafe extern "system" fn collect(_: HMONITOR, _: HDC, r: *mut RECT, out: LPARAM) -> BOOL {
            let list = &mut *(out.0 as *mut Vec<Rect>);
            let r = *r;
            list.push((r.left, r.top, r.right, r.bottom));
            BOOL(1)
        }
        let mut list: Vec<Rect> = Vec::new();
        unsafe {
            let _ = EnumDisplayMonitors(None, None, Some(collect), LPARAM(&mut list as *mut _ as isize));
        }
        list
    }

    /// The hook: a stream window still in its own frame (top-level) that is made, shown or moved
    /// onto a monitor goes out of sight at once, on the viewer's thread. Its own move comes back
    /// here, and finds it out of sight already.
    ///
    /// # Safety
    /// Called by Windows as a `WINEVENTPROC`.
    #[no_mangle]
    pub unsafe extern "system" fn expand_viewer_event(
        _hook: HWINEVENTHOOK,
        _event: u32,
        window: HWND,
        object: i32,
        child: i32,
        _thread: u32,
        _time: u32,
    ) {
        if object != OBJID_WINDOW.0 || child != CHILDID_SELF as i32 || window.0.is_null() {
            return;
        }
        let mut class = [0u16; 32];
        let len = GetClassNameW(window, &mut class).max(0) as usize;
        if String::from_utf16_lossy(&class[..len]) != STREAM_CLASS {
            return;
        }
        if GetWindowLongPtrW(window, GWL_STYLE) & WS_CHILD.0 as isize != 0 {
            return;
        }
        let mut r = RECT::default();
        if GetWindowRect(window, &mut r).is_err() {
            return;
        }
        if let Some((to_x, to_y)) = hiding_place((r.left, r.top, r.right, r.bottom), &monitors()) {
            // On the window's own thread (the event comes from inside its own call) the move is
            // done now. From any other thread it is only asked for: it never waits for the
            // window's thread.
            let own = GetWindowThreadProcessId(window, None) == GetCurrentThreadId();
            let flags = SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | if own { Default::default() } else { SWP_ASYNCWINDOWPOS };
            let _ = SetWindowPos(window, None, to_x, to_y, 0, 0, flags);
        }
    }
}

#[cfg(windows)]
pub use hook::expand_viewer_event;

#[cfg(test)]
mod tests {
    use super::*;

    // The owner's PC (physical pixels): monitor 2 at the origin (2560 × 1440, 125 %, 144 Hz),
    // monitor 1 left of it and lower (1920 × 1080, 100 %, 60 Hz).
    const M2: Rect = (0, 0, 2560, 1440);
    const M1: Rect = (-1920, 358, 0, 1438);
    const DESK: [Rect; 2] = [M2, M1];

    fn rect_at((x, y): (i32, i32), like: Rect) -> Rect {
        (x, y, x + like.2 - like.0, y + like.3 - like.1)
    }

    fn nearest(r: Rect, monitors: &[Rect]) -> Rect {
        let distance = |m: &Rect| {
            let dx = (m.0 - r.2).max(r.0 - m.2).max(0) as i64;
            let dy = (m.1 - r.3).max(r.1 - m.3).max(0) as i64;
            dx * dx + dy * dy
        };
        *monitors.iter().min_by_key(|m| distance(m)).unwrap()
    }

    #[test]
    fn a_window_waits_just_off_its_own_monitor_on_no_monitor() {
        // Where Moonlight's stream window was seen made, on each monitor (2026-11-24).
        for (window, mine) in [((231, 80, 1816, 1001), M2), ((-1701, 431, -220, 1294), M1)] {
            let at = hiding_place(window, &DESK).unwrap();
            let hidden = rect_at(at, window);
            assert!(DESK.iter().all(|m| overlap(grown(hidden, GAP - 1), *m) == 0), "{hidden:?} is on or near a monitor");
            // Windows still counts it on the monitor it was made on (its scale and refresh rate).
            assert_eq!(nearest(hidden, &DESK), mine, "{hidden:?}");
        }
    }

    #[test]
    fn above_its_monitor_first() {
        let window = (231, 80, 1816, 1001);
        assert_eq!(hiding_place(window, &DESK), Some((231, -GAP - 921)));
    }

    #[test]
    fn with_monitors_all_round_it_goes_left_of_them_all() {
        // A monitor above, below, left and right of the middle one.
        let middle = (0, 0, 1000, 1000);
        let around = [middle, (0, -1000, 1000, 0), (0, 1000, 1000, 2000), (-1000, 0, 0, 1000), (1000, 0, 2000, 1000)];
        let window = (100, 100, 600, 500);
        assert_eq!(hiding_place(window, &around), Some((-1000 - OUT_OF_SIGHT - 500, 100)));
    }

    #[test]
    fn a_window_on_no_monitor_stays_where_it_is() {
        // Where the hook put it: its own move comes back to the hook.
        let window = (231, 80, 1816, 1001);
        let hidden = rect_at(hiding_place(window, &DESK).unwrap(), window);
        assert_eq!(hiding_place(hidden, &DESK), None);
        // Touching an edge is not on it.
        assert_eq!(hiding_place((-2000, 358, -1920, 400), &DESK), None);
        assert_eq!(hiding_place(window, &[]), None);
    }

    /// Whether `r` is on none of this PC's monitors.
    #[cfg(windows)]
    fn on_no_monitor(r: Rect) -> bool {
        crate::hook::monitors().iter().all(|m| overlap(r, *m) == 0)
    }

    /// Called on a thread other than the window's (an event raised elsewhere, or no library in
    /// the viewer), the hook never waits for the window's thread: its move is only asked for,
    /// and lands once that thread is back at its messages.
    #[cfg(windows)]
    #[test]
    fn off_its_windows_thread_the_hook_never_waits_for_it() {
        use std::time::{Duration, Instant};
        use windows::core::w;
        use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
        use windows::Win32::UI::Accessibility::HWINEVENTHOOK;
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetWindowRect, PeekMessageW, RegisterClassW,
            CHILDID_SELF, EVENT_OBJECT_SHOW, MSG, OBJID_WINDOW, PM_REMOVE, WINDOW_EX_STYLE, WNDCLASSW, WS_OVERLAPPEDWINDOW,
        };
        unsafe extern "system" fn proc(window: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
            DefWindowProcW(window, msg, w, l)
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let busy = std::thread::spawn(move || unsafe {
            let wc = WNDCLASSW { lpfnWndProc: Some(proc), lpszClassName: w!("SDL_app"), ..Default::default() };
            RegisterClassW(&wc);
            let window = CreateWindowExW(WINDOW_EX_STYLE(0), w!("SDL_app"), w!(""), WS_OVERLAPPEDWINDOW, 100, 100, 640, 480, None, None, None, None).unwrap();
            tx.send(window.0 as isize).unwrap();
            // Busy, as Moonlight's thread is right after it makes its window.
            std::thread::sleep(Duration::from_millis(1500));
            let until = Instant::now() + Duration::from_secs(3);
            let mut msg = MSG::default();
            let mut r = RECT::default();
            while Instant::now() < until {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    DispatchMessageW(&msg);
                }
                GetWindowRect(window, &mut r).unwrap();
                if on_no_monitor((r.left, r.top, r.right, r.bottom)) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            let _ = DestroyWindow(window);
            (r.left, r.top, r.right, r.bottom)
        });
        let window = HWND(rx.recv().unwrap() as *mut _);
        let started = Instant::now();
        unsafe { expand_viewer_event(HWINEVENTHOOK::default(), EVENT_OBJECT_SHOW, window, OBJID_WINDOW.0, CHILDID_SELF as i32, 0, 0) };
        let took = started.elapsed();
        let landed_at = busy.join().unwrap();
        assert!(took < Duration::from_millis(500), "the hook waited {took:?} for the window's thread");
        assert!(on_no_monitor(landed_at), "the move never landed, the window is at {landed_at:?}");
    }

    /// The hook on a real window of this process: a stream window made on a monitor is out of
    /// sight the moment the hook returns (no message loop needed), and others are left alone.
    #[cfg(windows)]
    #[test]
    fn the_hook_puts_a_stream_window_out_of_sight_at_once() {
        use windows::core::{w, PCWSTR};
        use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
        use windows::Win32::UI::Accessibility::HWINEVENTHOOK;
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, GetWindowRect, RegisterClassW, CHILDID_SELF, EVENT_OBJECT_CREATE,
            OBJID_WINDOW, WINDOW_EX_STYLE, WNDCLASSW, WS_OVERLAPPEDWINDOW,
        };
        unsafe extern "system" fn proc(window: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
            DefWindowProcW(window, msg, w, l)
        }
        let make = |class: PCWSTR| unsafe {
            let wc = WNDCLASSW { lpfnWndProc: Some(proc), lpszClassName: class, ..Default::default() };
            RegisterClassW(&wc);
            // Hidden (no WS_VISIBLE): nothing shows.
            CreateWindowExW(WINDOW_EX_STYLE(0), class, w!(""), WS_OVERLAPPEDWINDOW, 100, 100, 640, 480, None, None, None, None).unwrap()
        };
        let rect = |w: HWND| unsafe {
            let mut r = RECT::default();
            GetWindowRect(w, &mut r).unwrap();
            (r.left, r.top, r.right, r.bottom)
        };
        let (stream, other) = (make(w!("SDL_app")), make(w!("ExpandHookTestOther")));
        for w in [stream, other] {
            unsafe { expand_viewer_event(HWINEVENTHOOK::default(), EVENT_OBJECT_CREATE, w, OBJID_WINDOW.0, CHILDID_SELF as i32, 0, 0) };
        }
        let (moved, left) = (rect(stream), rect(other));
        unsafe {
            let _ = DestroyWindow(stream);
            let _ = DestroyWindow(other);
        }
        assert!(on_no_monitor(moved), "the stream window is at {moved:?}");
        assert_eq!((left.0, left.1), (100, 100));
    }
}
