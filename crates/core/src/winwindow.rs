//! Windows-only glue: find the viewer window, read the screen layout, move the cursor.
//! The decisions live in `share::geometry`; this file only talks to Win32.

use crate::share::geometry::Area;
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, BOOL, HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, HDC, HMONITOR};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClientRect, GetCursorPos, GetForegroundWindow, GetWindowRect, SetForegroundWindow, ShowWindow, SW_HIDE, SW_SHOWNOACTIVATE, GetWindowTextLengthW, GetWindowThreadProcessId, IsWindowVisible,
    LoadIconW, SendMessageW, SetCursorPos, SetWindowPos, SetWindowTextW, ICON_BIG, ICON_SMALL, SWP_NOACTIVATE,
    SWP_NOZORDER, WM_SETICON,
};

/// The viewer's executable name.
pub const VIEWER_EXE: &str = "Moonlight.exe";
/// The icon resource Tauri embeds in our executable.
const APP_ICON_RESOURCE: usize = 32512;

fn hwnd(handle: isize) -> HWND {
    HWND(handle as *mut core::ffi::c_void)
}

fn to_area(r: RECT) -> Area {
    Area { left: r.left as f64, top: r.top as f64, right: r.right as f64, bottom: r.bottom as f64 }
}

/// Positions in physical pixels on every monitor, whatever its scaling. Call once at start.
pub fn use_physical_pixels() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

fn process_of(window: HWND) -> u32 {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(window, Some(&mut pid)) };
    pid
}

fn exe_name_of(pid: u32) -> Option<String> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buffer = [0u16; 1024];
        let mut len = buffer.len() as u32;
        let ok = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buffer.as_mut_ptr()), &mut len);
        let _ = CloseHandle(process);
        ok.ok()?;
        let path = String::from_utf16_lossy(&buffer[..len as usize]);
        path.rsplit(['\\', '/']).next().map(str::to_string)
    }
}

/// Every visible, titled top-level window, with the process that owns it.
fn top_level_windows() -> Vec<(isize, u32)> {
    unsafe extern "system" fn collect(window: HWND, out: LPARAM) -> BOOL {
        let list = &mut *(out.0 as *mut Vec<(isize, u32)>);
        if IsWindowVisible(window).as_bool() && GetWindowTextLengthW(window) > 0 {
            list.push((window.0 as isize, process_of(window)));
        }
        BOOL(1)
    }
    let mut list: Vec<(isize, u32)> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&mut list as *mut _ as isize));
    }
    list
}

fn area(window: isize) -> f64 {
    window_rect(window).map_or(0.0, |r| (r.right - r.left) * (r.bottom - r.top))
}

/// The largest visible window of a process with this executable name (case-insensitive).
pub fn find_window_of_process(exe_name: &str) -> Option<isize> {
    top_level_windows()
        .into_iter()
        .filter(|(_, pid)| exe_name_of(*pid).is_some_and(|name| name.eq_ignore_ascii_case(exe_name)))
        .map(|(window, _)| window)
        .max_by(|a, b| area(*a).total_cmp(&area(*b)))
}

/// The largest visible window of this process.
pub fn find_window_of_pid(pid: u32) -> Option<isize> {
    top_level_windows()
        .into_iter()
        .filter(|(_, p)| *p == pid)
        .map(|(w, _)| w)
        .max_by(|a, b| area(*a).total_cmp(&area(*b)))
}

pub fn window_rect(window: isize) -> Option<Area> {
    let mut r = RECT::default();
    unsafe { GetWindowRect(hwnd(window), &mut r).ok()? };
    Some(to_area(r))
}

/// What shows of a window: its rect less the invisible resize frame Windows (and Tauri's
/// frameless windows) keep round it. The whole rect when Windows can't tell.
pub fn visible_rect(window: isize) -> Option<Area> {
    use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
    let mut r = RECT::default();
    let size = std::mem::size_of::<RECT>() as u32;
    match unsafe { DwmGetWindowAttribute(hwnd(window), DWMWA_EXTENDED_FRAME_BOUNDS, &mut r as *mut RECT as _, size) } {
        Ok(()) if r.right > r.left && r.bottom > r.top => Some(to_area(r)),
        _ => window_rect(window),
    }
}

/// The inside of a window (no title bar or borders), in screen positions.
pub fn client_area(window: isize) -> Option<Area> {
    let mut r = RECT::default();
    let mut origin = POINT::default();
    unsafe {
        GetClientRect(hwnd(window), &mut r).ok()?;
        ClientToScreen(hwnd(window), &mut origin).ok().ok()?;
    }
    let (w, h) = ((r.right - r.left) as f64, (r.bottom - r.top) as f64);
    let (x, y) = (origin.x as f64, origin.y as f64);
    Some(Area { left: x, top: y, right: x + w, bottom: y + h })
}

pub fn monitors() -> Vec<Area> {
    unsafe extern "system" fn collect(_: HMONITOR, _: HDC, rect: *mut RECT, out: LPARAM) -> BOOL {
        let list = &mut *(out.0 as *mut Vec<Area>);
        list.push(to_area(*rect));
        BOOL(1)
    }
    let mut list: Vec<Area> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(collect), LPARAM(&mut list as *mut _ as isize));
    }
    list
}

pub fn cursor() -> Option<(f64, f64)> {
    let mut p = POINT::default();
    unsafe { GetCursorPos(&mut p).ok()? };
    Some((p.x as f64, p.y as f64))
}

pub fn set_cursor((x, y): (f64, f64)) {
    unsafe {
        let _ = SetCursorPos(x.round() as i32, y.round() as i32);
    }
}

/// The window in front, the one keyboard input goes to.
pub fn foreground_window() -> isize {
    unsafe { GetForegroundWindow().0 as isize }
}

/// Shows a hidden window without making it the one keyboard input goes to.
pub fn show_without_focus(window: isize) {
    unsafe {
        let _ = ShowWindow(hwnd(window), SW_SHOWNOACTIVATE);
    }
}

pub fn hide(window: isize) {
    unsafe {
        let _ = ShowWindow(hwnd(window), SW_HIDE);
    }
}

/// Hides a window without waiting for the thread it belongs to (another app's, which may hang).
pub fn hide_soon(window: isize) {
    use windows::Win32::UI::WindowsAndMessaging::ShowWindowAsync;
    unsafe {
        let _ = ShowWindowAsync(hwnd(window), SW_HIDE);
    }
}

pub fn is_visible(window: isize) -> bool {
    unsafe { IsWindowVisible(hwnd(window)).as_bool() }
}

/// The window with the keyboard focus on the thread of the window in front (0 for none). An
/// embedded viewer window shares that thread's input, so it is found there too.
fn keyboard_focus(front: HWND) -> isize {
    use windows::Win32::UI::WindowsAndMessaging::{GetGUIThreadInfo, GUITHREADINFO};
    let mut info = GUITHREADINFO { cbSize: std::mem::size_of::<GUITHREADINFO>() as u32, ..Default::default() };
    unsafe {
        let thread = GetWindowThreadProcessId(front, None);
        if thread == 0 || GetGUIThreadInfo(thread, &mut info).is_err() {
            return 0;
        }
    }
    info.hwndFocus.0 as isize
}

/// If one of this app's windows is in front, puts the desktop in front instead, unless the
/// keyboard is on the embedded viewer window
/// ([`should_step_aside`](crate::share::geometry::should_step_aside)).
pub fn step_aside_if_in_front() {
    use crate::share::geometry::should_step_aside;
    use windows::Win32::UI::WindowsAndMessaging::GetShellWindow;
    unsafe {
        let front = GetForegroundWindow();
        let ours = process_of(front) == std::process::id();
        let embedded = EMBEDDED_VIEWER.load(std::sync::atomic::Ordering::Relaxed);
        if should_step_aside(ours, keyboard_focus(front), embedded) {
            let _ = SetForegroundWindow(GetShellWindow());
        }
    }
}

pub fn bring_to_front(window: isize) {
    unsafe {
        let _ = SetForegroundWindow(hwnd(window));
    }
}

/// Whether clicking one of this app's windows makes it the window in front (and gives it the
/// keyboard). Off (`WS_EX_NOACTIVATE`), clicks still reach it, and the window in front stays.
/// Set through Windows directly: a window shown and hidden behind tao's back must not go through
/// tao's style updates, which also apply tao's own idea of whether the window is visible.
pub fn set_activatable(window: isize, activatable: bool) {
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowLongPtrW, SetWindowLongPtrW, GWL_EXSTYLE, WS_EX_NOACTIVATE};
    unsafe {
        let ex = GetWindowLongPtrW(hwnd(window), GWL_EXSTYLE);
        let no_activate = WS_EX_NOACTIVATE.0 as isize;
        let new = if activatable { ex & !no_activate } else { ex | no_activate };
        if new != ex {
            SetWindowLongPtrW(hwnd(window), GWL_EXSTYLE, new);
        }
    }
}

/// The keyboard layout of the window in front (a layout handle, see `language_tag`).
pub fn keyboard_layout() -> usize {
    use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyboardLayout;
    unsafe {
        let front = GetForegroundWindow();
        GetKeyboardLayout(GetWindowThreadProcessId(front, None)).0 as usize
    }
}

/// Switches the window in front to the installed keyboard layout after `current` (what
/// Alt+Shift does) and returns it. The switch lands a moment later, so the caller keeps track
/// of the layout rather than reading it back.
pub fn switch_keyboard_layout(current: usize) -> Option<usize> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyboardLayoutList, HKL};
    use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_INPUTLANGCHANGEREQUEST};
    unsafe {
        let mut layouts = [HKL::default(); 32];
        let count = GetKeyboardLayoutList(Some(&mut layouts)).max(0) as usize;
        let layouts = &layouts[..count];
        let at = layouts.iter().position(|l| l.0 as usize == current).unwrap_or(0);
        let next = *layouts.get((at + 1) % count.max(1))?;
        let _ = PostMessageW(GetForegroundWindow(), WM_INPUTLANGCHANGEREQUEST, WPARAM(0), LPARAM(next.0 as isize));
        Some(next.0 as usize)
    }
}

/// The language part of a keyboard layout handle, as a tag like "en-US".
pub fn language_tag(layout: usize) -> Option<String> {
    use windows::Win32::Globalization::LCIDToLocaleName;
    let lcid = (layout & 0xFFFF) as u32;
    let mut name = [0u16; 85];
    let len = unsafe { LCIDToLocaleName(lcid, Some(&mut name), 0) };
    (len > 1).then(|| String::from_utf16_lossy(&name[..len as usize - 1]))
}

/// Moves and sizes a window, without changing which window is in front.
pub fn move_window(window: isize, to: Area) {
    let (x, y) = (to.left.round() as i32, to.top.round() as i32);
    let (w, h) = ((to.right - to.left).round() as i32, (to.bottom - to.top).round() as i32);
    unsafe {
        let _ = SetWindowPos(hwnd(window), None, x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

/// Makes the viewer window read as part of this app: our title and our icon.
pub fn brand_window(window: isize, title: &str) {
    let wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let _ = SetWindowTextW(hwnd(window), PCWSTR(wide.as_ptr()));
        let Ok(module) = GetModuleHandleW(None) else { return };
        if let Ok(icon) = LoadIconW(module, PCWSTR(APP_ICON_RESOURCE as *const u16)) {
            for size in [ICON_BIG, ICON_SMALL] {
                SendMessageW(hwnd(window), WM_SETICON, WPARAM(size as usize), LPARAM(icon.0 as isize));
            }
        }
    }
}

fn class_name(window: isize) -> String {
    use windows::Win32::UI::WindowsAndMessaging::GetClassNameW;
    let mut buffer = [0u16; 256];
    let len = unsafe { GetClassNameW(hwnd(window), &mut buffer) }.max(0) as usize;
    String::from_utf16_lossy(&buffer[..len])
}

use crate::layout::STREAM_CLASS;

/// The viewer's stream window: the largest visible SDL window of this process. Its other windows
/// (the Qt launcher shown while the stream starts, a message box) are never it.
pub fn find_stream_window_of_pid(pid: u32) -> Option<isize> {
    top_level_windows()
        .into_iter()
        .filter(|(w, p)| *p == pid && class_name(*w) == STREAM_CLASS)
        .map(|(w, _)| w)
        .max_by(|a, b| area(*a).total_cmp(&area(*b)))
}

/// Whether `window` is of the viewer's stream window's class (in its own frame or embedded).
/// Reads only.
pub fn is_stream_window(window: isize) -> bool {
    class_name(window) == STREAM_CLASS
}

/// Asks a window of another process to move to (`x`, `y`), keeping its size and z-order, without
/// waiting for it (from a thread that shares input with no one, Windows hands it over).
fn ask_window_to_move(window: isize, x: f64, y: f64) {
    use windows::Win32::UI::WindowsAndMessaging::{SWP_ASYNCWINDOWPOS, SWP_NOSIZE};
    let flags = SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS;
    unsafe {
        let _ = SetWindowPos(hwnd(window), None, x.round() as i32, y.round() as i32, 0, 0, flags);
    }
}

/// Watches a viewer's process windows while it runs: keeps its stream window out of sight from the
/// moment it is made until it is embedded (moved left of every monitor,
/// [`out_of_sight`](crate::layout::out_of_sight)), and tells when the viewer speaks to the user
/// (a window it speaks in shows: its launcher with an error, a message box,
/// [`viewer_act`](crate::layout::viewer_act)), so that is brought to the user at once, never
/// hidden. Its helpers are left alone. The stream window is moved rather than hidden: the viewer's
/// idea of it stays as it was.
///
/// It hears of them from Windows (`SetWinEventHook` on the viewer's process, out of context) on a
/// thread of its own, which only reads windows and asks them to move (never waiting for the
/// viewer). Out of context, a window can be on a monitor for the moment between Windows showing
/// it and the viewer's thread doing the move (in context would need a library of ours loaded into
/// the viewer, which ships an anti-hooking guard); the trace ([`crate::trace`]) measures that
/// moment. [`reveal`](Concealer::reveal) gives back the stream windows not embedded where the
/// viewer last put them, and shows the launcher a hidden start kept hidden (the fallback);
/// dropped, it just stops.
pub struct Concealer {
    thread: u32,
    handle: Option<std::thread::JoinHandle<()>>,
}

/// Which of the viewer's windows it speaks to the user in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Talkers {
    /// Its launcher and message boxes ([`viewer_kind`](crate::layout::viewer_kind)).
    Viewer,
    /// Those of this class (tests, whose process has other windows).
    OfClass(&'static str),
}

impl Talkers {
    fn kind(self, class: &str) -> crate::layout::ViewerKind {
        use crate::layout::{viewer_kind, ViewerKind, STREAM_CLASS};
        match self {
            Talkers::Viewer => viewer_kind(class),
            Talkers::OfClass(_) if class == STREAM_CLASS => ViewerKind::Stream,
            Talkers::OfClass(c) if class == c => ViewerKind::Talk,
            Talkers::OfClass(_) => ViewerKind::Helper,
        }
    }
}

/// What the concealer tells, from its thread (whoever hears it must not wait).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Heard {
    /// A stream window showed (so it is embedded at once).
    StreamShown(isize),
    /// The viewer speaks to the user in this window, which just showed.
    Speaks(isize),
}

/// Asks the concealer's thread to give its windows back, and stop.
const REVEAL: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 0x53;

/// What a concealer's thread works with.
struct Watch {
    pid: u32,
    talkers: Talkers,
    tell: Option<Box<dyn Fn(Heard)>>,
    /// Windows it speaks in that showed since they last hid (each is told of once per showing),
    /// and all that ever showed (a launcher that never did was kept hidden by the start).
    speaking: std::collections::HashSet<isize>,
    spoke: std::collections::HashSet<isize>,
    /// A stream window showed (its launcher was then hidden for the stream, not by the start).
    stream_seen: bool,
    /// How long each window was on a monitor, for the trace.
    exposure: crate::trace::Exposure,
}

thread_local! {
    /// On a concealer's thread: the windows it keeps out of sight, with where the viewer last put
    /// them.
    /// (`None`: the in-context hook moved it before it was seen in sight, from where is not known.)
    static CONCEALED: std::cell::RefCell<std::collections::HashMap<isize, Option<Area>>> = std::cell::RefCell::new(std::collections::HashMap::new());
    static WATCH: std::cell::RefCell<Option<Watch>> = const { std::cell::RefCell::new(None) };
}

impl Concealer {
    /// Starts watching process `pid`'s windows (a run of the viewer before its stream, which
    /// makes no stream window: no in-context hook goes into it); returns once Windows tells of
    /// them.
    pub fn start(pid: u32) -> Concealer {
        Concealer::start_with(pid, Talkers::Viewer, None, false)
    }

    /// For the stream's viewer: [`start`](Concealer::start), and tells `tell` what it hears
    /// ([`Heard`]); `in_context`: also with the in-context hook ([`in_context_hooks`]), so its
    /// stream window is out of sight from the moment it is made.
    pub fn start_telling(pid: u32, in_context: bool, tell: impl Fn(Heard) + Send + 'static) -> Concealer {
        Concealer::start_with(pid, Talkers::Viewer, Some(Box::new(tell)), in_context)
    }

    fn start_with(pid: u32, talkers: Talkers, tell: Option<Box<dyn Fn(Heard) + Send>>, in_context: bool) -> Concealer {
        let (tx, rx) = std::sync::mpsc::channel();
        let handle = std::thread::Builder::new()
            .name("expand-conceal-viewer".into())
            .spawn(move || {
                let tell = tell.map(|f| Box::new(move |h| f(h)) as Box<dyn Fn(Heard)>);
                WATCH.with(|w| {
                    *w.borrow_mut() = Some(Watch {
                        pid,
                        talkers,
                        tell,
                        speaking: Default::default(),
                        spoke: Default::default(),
                        stream_seen: false,
                        exposure: Default::default(),
                    })
                });
                conceal(pid, tx, in_context)
            })
            .ok();
        let thread = if handle.is_some() { rx.recv().unwrap_or(0) } else { 0 };
        Concealer { thread, handle }
    }

    /// Gives every stream window still in its own frame back where the viewer last put it (in
    /// sight again), shows the launcher a hidden start kept hidden, and stops.
    pub fn reveal(mut self) {
        self.stop(REVEAL);
    }

    fn stop(&mut self, message: u32) {
        use windows::Win32::UI::WindowsAndMessaging::PostThreadMessageW;
        let Some(handle) = self.handle.take() else { return };
        if unsafe { PostThreadMessageW(self.thread, message, WPARAM(0), LPARAM(0)) }.is_ok() {
            let _ = handle.join();
        }
    }
}

impl Drop for Concealer {
    fn drop(&mut self) {
        self.stop(windows::Win32::UI::WindowsAndMessaging::WM_QUIT);
    }
}

/// Whether `window` is still in its own frame: top-level, and not the embedded viewer. Reads
/// only (no message is sent).
fn is_free_window(window: isize) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowLongPtrW, GWL_STYLE, WS_CHILD};
    let child = unsafe { GetWindowLongPtrW(hwnd(window), GWL_STYLE) } & WS_CHILD.0 as isize != 0;
    !child && window != EMBEDDED_VIEWER.load(std::sync::atomic::Ordering::Relaxed)
}

/// The trace's name for a window event.
fn event_name(event: u32) -> &'static str {
    use windows::Win32::UI::WindowsAndMessaging::{
        EVENT_OBJECT_CREATE, EVENT_OBJECT_DESTROY, EVENT_OBJECT_HIDE, EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_SHOW,
    };
    match event {
        e if e == EVENT_OBJECT_CREATE => "create",
        e if e == EVENT_OBJECT_DESTROY => "destroy",
        e if e == EVENT_OBJECT_SHOW => "show",
        e if e == EVENT_OBJECT_HIDE => "hide",
        e if e == EVENT_OBJECT_LOCATIONCHANGE => "move",
        _ => "event",
    }
}

/// The viewer's process and how long `window` had been on a monitor, now that it is `showing` or
/// not, at `at` (Windows' own ms), for the trace.
fn exposure(window: isize, at: u32, showing: bool) -> (u32, String) {
    WATCH.with(|w| match w.borrow_mut().as_mut() {
        Some(w) => {
            let showed = w.exposure.seen(window, at as u64, showing);
            (w.pid, showed.map_or(String::new(), |ms| format!("; it showed for {ms} ms")))
        }
        None => (0, String::new()),
    })
}

fn tell(heard: Heard) {
    WATCH.with(|w| {
        if let Some(tell) = w.borrow().as_ref().and_then(|w| w.tell.as_ref()) {
            tell(heard);
        }
    });
}

/// Window `window` of the viewer was made, shown, hidden or moved (`event`, at `at`, Windows'
/// own ms): what [`viewer_act`](crate::layout::viewer_act) says is done; a stream window that
/// shows is told of, and so is a window the viewer speaks in, once each time it shows.
fn conceal_window(window: isize, event: u32, at: u32) {
    use crate::layout::{out_of_sight, shows_on, viewer_act, ViewerAct, ViewerKind, ViewerWindow};
    let Some(rect) = window_rect(window) else { return };
    let monitors = monitors();
    let class = class_name(window);
    let talkers = WATCH.with(|w| w.borrow().as_ref().map_or(Talkers::Viewer, |w| w.talkers));
    let seen = ViewerWindow {
        free: is_free_window(window),
        kind: talkers.kind(&class),
        visible: is_visible(window),
        on_a_monitor: shows_on(rect, &monitors),
    };
    let act = viewer_act(seen);
    if crate::trace::on() {
        // On a monitor and shown in its own frame: what the user sees (until a move lands).
        let showing = seen.free && seen.visible && seen.on_a_monitor;
        let (pid, showed) = exposure(window, at, showing && act != ViewerAct::Conceal);
        crate::trace::line(|| format!(
            "viewer {pid} {} {window:#x} class {class} visible {} at {},{} {}x{} free {} -> {:?}{showed}",
            event_name(event),
            seen.visible as u8,
            rect.left,
            rect.top,
            rect.right - rect.left,
            rect.bottom - rect.top,
            seen.free as u8,
            act,
        ));
    }
    if seen.kind == ViewerKind::Stream && seen.free && seen.visible {
        WATCH.with(|w| {
            if let Some(w) = w.borrow_mut().as_mut() {
                w.stream_seen = true;
            }
        });
        tell(Heard::StreamShown(window));
    }
    if seen.kind == ViewerKind::Stream && seen.free && !seen.on_a_monitor {
        // Out of sight already: the in-context hook moved it, on the viewer's thread.
        CONCEALED.with(|c| {
            c.borrow_mut().entry(window).or_insert(None);
        });
    }
    match act {
        ViewerAct::Conceal => {
            CONCEALED.with(|c| c.borrow_mut().insert(window, Some(rect)));
            let to = out_of_sight(rect, &monitors);
            // Read again right before asking: the window may have been embedded meanwhile. (A
            // move asked just before an embed can still land after it; the display places its
            // window again after an embed, which undoes it.)
            if is_free_window(window) {
                ask_window_to_move(window, to.left, to.top);
            }
        }
        ViewerAct::Speaks => {
            let new = WATCH.with(|w| {
                w.borrow_mut().as_mut().is_some_and(|w| {
                    w.spoke.insert(window);
                    w.speaking.insert(window)
                })
            });
            if new {
                tell(Heard::Speaks(window));
            }
        }
        ViewerAct::Leave => {
            if !seen.visible {
                WATCH.with(|w| {
                    if let Some(w) = w.borrow_mut().as_mut() {
                        w.speaking.remove(&window);
                    }
                });
            }
        }
    }
}

unsafe extern "system" fn window_event(
    _: windows::Win32::UI::Accessibility::HWINEVENTHOOK,
    event: u32,
    window: HWND,
    object: i32,
    child: i32,
    _: u32,
    at: u32,
) {
    use windows::Win32::UI::WindowsAndMessaging::{CHILDID_SELF, EVENT_OBJECT_DESTROY, OBJID_WINDOW};
    if object != OBJID_WINDOW.0 || child != CHILDID_SELF as i32 {
        return;
    }
    let window = window.0 as isize;
    if event == EVENT_OBJECT_DESTROY {
        CONCEALED.with(|c| c.borrow_mut().remove(&window));
        WATCH.with(|w| {
            if let Some(w) = w.borrow_mut().as_mut() {
                w.speaking.remove(&window);
            }
        });
        if crate::trace::on() {
            let (pid, showed) = exposure(window, at, false);
            crate::trace::line(|| format!("viewer {pid} destroy {window:#x}{showed}"));
        }
    } else {
        conceal_window(window, event, at);
    }
}

/// The concealer's thread: hooks process `pid`'s window events, tells `ready` its thread id, and
/// serves the events until it is asked to stop.
fn conceal(pid: u32, ready: std::sync::mpsc::Sender<u32>, in_context: bool) {
    use windows::Win32::System::SystemInformation::GetTickCount;
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent};
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, PeekMessageW, ShowWindowAsync, EVENT_OBJECT_CREATE, EVENT_OBJECT_HIDE,
        EVENT_OBJECT_LOCATIONCHANGE, MSG, PM_NOREMOVE, SW_SHOW, WINEVENT_OUTOFCONTEXT,
    };
    let mut msg = MSG::default();
    let mut reveal = false;
    unsafe {
        // A message queue, so the stop request can be posted from now on.
        let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
        // Made, destroyed, shown, hidden.
        let made = SetWinEventHook(EVENT_OBJECT_CREATE, EVENT_OBJECT_HIDE, None, Some(window_event), pid, 0, WINEVENT_OUTOFCONTEXT);
        let moved = SetWinEventHook(
            EVENT_OBJECT_LOCATIONCHANGE,
            EVENT_OBJECT_LOCATIONCHANGE,
            None,
            Some(window_event),
            pid,
            0,
            WINEVENT_OUTOFCONTEXT,
        );
        // And on the viewer's own thread, inside the calls that make, show and move its windows:
        // its stream window is out of sight before Windows draws it anywhere.
        let inside = if in_context {
            in_context_hooks(pid)
        } else {
            crate::trace::line(|| format!("viewer {pid} no in-context hook asked for"));
            Vec::new()
        };
        let _ = ready.send(GetCurrentThreadId());
        let in_context = !inside.is_empty();
        crate::trace::line(|| format!("viewer {pid} watched{}", if in_context { ", also on its own thread" } else { "" }));
        // Those made before the hooks were set.
        for window in windows_of_process(pid) {
            conceal_window(window, EVENT_OBJECT_CREATE, GetTickCount());
        }
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if msg.message == REVEAL {
                reveal = true;
                break;
            }
            DispatchMessageW(&msg);
        }
        for hook in inside {
            let _ = UnhookWinEvent(hook);
        }
        let _ = UnhookWinEvent(made);
        let _ = UnhookWinEvent(moved);
    }
    let concealed = CONCEALED.with(|c| std::mem::take(&mut *c.borrow_mut()));
    if reveal {
        for (window, was) in concealed {
            if is_window(window) && is_free_window(window) {
                crate::trace::line(|| format!("viewer {pid} given back {window:#x}"));
                match was.or_else(|| window_rect(window).and_then(|r| in_sight_place(r, &monitors()))) {
                    Some(was) => ask_window_to_move(window, was.left, was.top),
                    None => {}
                }
            }
        }
        // The launcher a hidden start kept hidden (`moonlight::hidden`): the viewer thinks it
        // shows, so it never shows it again by itself, and its errors are in it. Not once a
        // stream window showed (the viewer hides its launcher for the stream), nor one the viewer
        // showed itself since (and then hid again).
        let (talkers, spoke, stream_seen) = WATCH.with(|w| {
            w.borrow().as_ref().map_or((Talkers::Viewer, Default::default(), false), |w| (w.talkers, w.spoke.clone(), w.stream_seen))
        });
        for window in windows_of_process(pid).into_iter().filter(|_| !stream_seen) {
            let class = class_name(window);
            let launcher = talkers.kind(&class) == crate::layout::ViewerKind::Talk;
            if launcher && is_free_window(window) && !is_visible(window) && !spoke.contains(&window) {
                crate::trace::line(|| format!("viewer {pid} shown again {window:#x} class {class}"));
                unsafe {
                    let _ = ShowWindowAsync(hwnd(window), SW_SHOW);
                }
            }
        }
    }
    crate::trace::line(|| format!("viewer {pid} no longer watched"));
}

/// The library with the in-context hook (`expand_viewer_hook.dll`, beside this program) and
/// its hook, once loaded (tried again each time until it is); `None` when it can't be (the
/// out-of-context hook then does it all, a moment late). The trace says why.
fn viewer_hook_library() -> Option<(isize, usize)> {
    static LIBRARY: std::sync::Mutex<Option<(isize, usize)>> = std::sync::Mutex::new(None);
    let mut library = LIBRARY.lock().unwrap();
    if library.is_none() {
        match load_viewer_hook() {
            Ok(loaded) => *library = Some(loaded),
            Err(why) => crate::trace::line(|| format!("viewer hook library: {why}")),
        }
    }
    *library
}

fn load_viewer_hook() -> Result<(isize, usize), String> {
    use windows::core::{s, HSTRING};
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    let exe = std::env::current_exe().map_err(|e| format!("no path for this program: {e}"))?;
    let path = exe.with_file_name("expand_viewer_hook.dll");
    // Loaded straight away (no check first): the error says what is wrong (2 or 126 not there or
    // a library it needs missing, 5 no access, 193 another machine's), with what this process
    // sees of the file.
    unsafe {
        let module = LoadLibraryW(&HSTRING::from(path.as_os_str())).map_err(|e| {
            let seen = match std::fs::metadata(&path) {
                Ok(m) => format!("it is there, {} bytes", m.len()),
                Err(why) => format!("this process doesn't see it: {why}"),
            };
            format!("{} won't load: error {} ({}); {seen}", path.display(), e.code().0 & 0xFFFF, e.message())
        })?;
        let hook = GetProcAddress(module, s!("expand_viewer_event"))
            .ok_or_else(|| format!("{} has no expand_viewer_event: {:?}", path.display(), windows::Win32::Foundation::GetLastError()))?;
        crate::trace::line(|| format!("viewer hook library: {} loaded", path.display()));
        Ok((module.0 as isize, hook as usize))
    }
}

/// Sets the in-context hooks on process `pid` (made, shown, moved), whose library Windows loads
/// into it ([`viewer_hook_library`]); none when that can't be done.
fn in_context_hooks(pid: u32) -> Vec<windows::Win32::UI::Accessibility::HWINEVENTHOOK> {
    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::UI::Accessibility::{SetWinEventHook, WINEVENTPROC};
    use windows::Win32::UI::WindowsAndMessaging::{
        EVENT_OBJECT_CREATE, EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_SHOW, WINEVENT_INCONTEXT,
    };
    // 0 would mean every process on the desktop.
    if pid == 0 {
        return Vec::new();
    }
    let Some((module, hook)) = viewer_hook_library() else { return Vec::new() };
    let module = HMODULE(module as *mut core::ffi::c_void);
    // The library's `expand_viewer_event` is a `WINEVENTPROC`.
    let proc: WINEVENTPROC = unsafe { std::mem::transmute::<usize, WINEVENTPROC>(hook) };
    let hooks: Vec<_> = [EVENT_OBJECT_CREATE, EVENT_OBJECT_SHOW, EVENT_OBJECT_LOCATIONCHANGE]
        .into_iter()
        .filter_map(|event| {
            let hook = unsafe { SetWinEventHook(event, event, module, proc, pid, 0, WINEVENT_INCONTEXT) };
            if hook.is_invalid() {
                let why = unsafe { windows::Win32::Foundation::GetLastError() };
                crate::trace::line(|| format!("viewer {pid} in-context hook for event {event:#x} not set: {why:?}"));
                None
            } else {
                Some(hook)
            }
        })
        .collect();
    crate::trace::line(|| format!("viewer {pid} in-context hooks set: {} of 3", hooks.len()));
    hooks
}

/// Where a window of `window`'s size goes back in sight when where it was is not known: centred
/// on the monitor at the screen's origin (the main one), else the first.
fn in_sight_place(window: Area, monitors: &[Area]) -> Option<Area> {
    let m = monitors.iter().find(|m| m.contains((0.0, 0.0))).or(monitors.first())?;
    let (w, h) = (window.right - window.left, window.bottom - window.top);
    let left = m.left + ((m.right - m.left - w) / 2.0).max(0.0);
    let top = m.top + ((m.bottom - m.top - h) / 2.0).max(0.0);
    Some(Area { left, top, right: left + w, bottom: top + h })
}

/// Every top-level window of process `pid`, shown or not.
fn windows_of_process(pid: u32) -> Vec<isize> {
    unsafe extern "system" fn collect(window: HWND, out: LPARAM) -> BOOL {
        let list = &mut *(out.0 as *mut Vec<(isize, u32)>);
        list.push((window.0 as isize, process_of(window)));
        BOOL(1)
    }
    let mut list: Vec<(isize, u32)> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&mut list as *mut _ as isize));
    }
    list.into_iter().filter(|(_, p)| *p == pid).map(|(w, _)| w).collect()
}

/// The window the viewer `pid` speaks in now: one of its launchers or message boxes still in its
/// own frame, a shown one first (else the launcher a hidden start kept hidden). Reads only.
pub fn talking_window_of(pid: u32) -> Option<isize> {
    use crate::layout::{viewer_kind, ViewerKind};
    let talking: Vec<isize> = windows_of_process(pid)
        .into_iter()
        .filter(|w| viewer_kind(&class_name(*w)) == ViewerKind::Talk && is_free_window(*w))
        .collect();
    talking.iter().copied().find(|w| is_visible(*w)).or_else(|| talking.first().copied())
}

/// Whether the viewer `pid` shows a window of its own on a monitor, in its own frame, other than
/// the ones it speaks in and its helpers: its stream in a window not known as the stream's (or
/// one not put out of sight). Reads only.
pub fn shows_own_window(pid: u32) -> bool {
    use crate::layout::{shows_on, viewer_kind, ViewerKind};
    let monitors = monitors();
    top_level_windows().into_iter().any(|(w, p)| {
        p == pid
            && viewer_kind(&class_name(w)) != ViewerKind::Talk
            && is_free_window(w)
            && window_rect(w).is_some_and(|r| shows_on(r, &monitors) && r.right - r.left >= 320.0 && r.bottom - r.top >= 200.0)
    })
}

/// Whether `window` is a message box (its text can be read, [`message_text`]). Reads only.
pub fn is_message_box(window: isize) -> bool {
    class_name(window) == crate::layout::MESSAGE_BOX_CLASS
}

/// The text of another process's message box: its static texts, one per line; `None` when there
/// is none, or its thread doesn't answer within half a second. Waits for that thread: not on the
/// app's main thread.
pub fn message_text(window: isize) -> Option<String> {
    use windows::Win32::UI::WindowsAndMessaging::{EnumChildWindows, SendMessageTimeoutW, SMTO_ABORTIFHUNG, WM_GETTEXT};
    unsafe extern "system" fn collect(child: HWND, out: LPARAM) -> BOOL {
        let list = &mut *(out.0 as *mut Vec<isize>);
        list.push(child.0 as isize);
        BOOL(1)
    }
    let mut children: Vec<isize> = Vec::new();
    unsafe {
        let _ = EnumChildWindows(hwnd(window), Some(collect), LPARAM(&mut children as *mut _ as isize));
    }
    let mut lines: Vec<String> = Vec::new();
    for child in children.into_iter().filter(|c| class_name(*c).eq_ignore_ascii_case("Static")) {
        let mut buffer = [0u16; 1024];
        let mut copied = 0usize;
        let answered = unsafe {
            SendMessageTimeoutW(
                hwnd(child),
                WM_GETTEXT,
                WPARAM(buffer.len()),
                LPARAM(buffer.as_mut_ptr() as isize),
                SMTO_ABORTIFHUNG,
                500,
                Some(&mut copied),
            )
        };
        if answered.0 == 0 {
            return None;
        }
        let text = String::from_utf16_lossy(&buffer[..copied.min(buffer.len())]).trim().to_string();
        if !text.is_empty() {
            lines.push(text);
        }
    }
    (!lines.is_empty()).then(|| lines.join("\n"))
}

/// Brings a window of another process to the user: shown, in front and with the keyboard; with
/// `over`, centred over that (the display window), on its monitor. Never waits for its thread.
pub fn bring_to_user(window: isize, over: Option<Area>) {
    use windows::Win32::UI::WindowsAndMessaging::{AllowSetForegroundWindow, ShowWindowAsync, SW_SHOW};
    if let (Some(over), Some(r)) = (over, window_rect(window)) {
        let (w, h) = (r.right - r.left, r.bottom - r.top);
        let x = over.left + ((over.right - over.left - w) / 2.0).max(0.0);
        let y = over.top + ((over.bottom - over.top - h) / 2.0).max(0.0);
        ask_window_to_move(window, x, y);
    }
    unsafe {
        let _ = AllowSetForegroundWindow(process_of(hwnd(window)));
        let _ = ShowWindowAsync(hwnd(window), SW_SHOW);
        let _ = SetForegroundWindow(hwnd(window));
    }
    crate::trace::line(|| format!("viewer window {window:#x} brought to the user"));
}

/// What a window is, for the log: its class name.
pub fn describe(window: isize) -> String {
    format!("{:#x} class {}", window, class_name(window))
}

pub fn is_window(window: isize) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::IsWindow;
    window != 0 && unsafe { IsWindow(hwnd(window)).as_bool() }
}

/// The viewer window while it is embedded in the display window (it is then no longer a
/// top-level window, so the process lookups do not find it).
static EMBEDDED_VIEWER: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

/// Records the embedded viewer window, or that there is none.
pub fn set_embedded_viewer(window: Option<isize>) {
    EMBEDDED_VIEWER.store(window.unwrap_or(0), std::sync::atomic::Ordering::Relaxed);
}

/// The process of the viewer whose window goes into the display window, 0 for none.
static VIEWER_PROCESS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Records the process of the viewer whose window goes into the display window (from its start
/// until it is stopped), or that there is none.
pub fn set_viewer_process(pid: Option<u32>) {
    VIEWER_PROCESS.store(pid.unwrap_or(0), std::sync::atomic::Ordering::Relaxed);
}

/// Forgets the viewer's process `pid` (it ended, or it is no longer the one shown), unless
/// another one was recorded since: a run that ends late never forgets a newer one.
pub fn forget_viewer_process(pid: u32) {
    use std::sync::atomic::Ordering;
    let _ = VIEWER_PROCESS.compare_exchange(pid, 0, Ordering::Relaxed, Ordering::Relaxed);
}

/// The process of the viewer whose window goes into the display window, while it runs.
pub fn viewer_process() -> Option<u32> {
    Some(VIEWER_PROCESS.load(std::sync::atomic::Ordering::Relaxed)).filter(|pid| *pid != 0)
}

/// The process a window belongs to (0 for none). Reads only.
pub fn process_of_window(window: isize) -> u32 {
    if window == 0 {
        return 0;
    }
    process_of(hwnd(window))
}

/// The viewer's window while it shows the picture: the embedded one while there is one, else its
/// own top-level window. `None` while the window it is in (the display window, or its own) is
/// hidden or minimized: a minimized window sits at -32000, far from any picture.
pub fn viewer_window() -> Option<isize> {
    use windows::Win32::UI::WindowsAndMessaging::{GetAncestor, GA_ROOT};
    let embedded = EMBEDDED_VIEWER.load(std::sync::atomic::Ordering::Relaxed);
    let window = if is_window(embedded) { embedded } else { find_window_of_process(VIEWER_EXE)? };
    let top = unsafe { GetAncestor(hwnd(window), GA_ROOT) }.0 as isize;
    let top = if top == 0 { window } else { top };
    (is_visible(top) && !is_minimized(top)).then_some(window)
}

/// Makes `child` (another process's top-level window) a frameless child of `parent`. Returns its
/// old style, for [`unembed`]. On failure the window is left as it was. Waits for `child`'s thread
/// (its style changes and the new parent are sent to it): not on the app's main thread, where a
/// busy viewer would hang the app.
pub fn embed(child: isize, parent: isize) -> Result<isize, String> {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetParent, GetWindowLongPtrW, SetParent, SetWindowLongPtrW, GWL_EXSTYLE, GWL_STYLE, WS_CHILD,
        WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_EX_NOPARENTNOTIFY, WS_VISIBLE,
    };
    if !is_window(child) || !is_window(parent) {
        return Err("the window is gone".to_string());
    }
    unsafe {
        let old_style = GetWindowLongPtrW(hwnd(child), GWL_STYLE);
        let style = (WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS).0 as isize;
        SetWindowLongPtrW(hwnd(child), GWL_STYLE, style);
        // The previous parent of a top-level window can read as none, so the result is checked
        // by asking for the new parent.
        // Clicks on it must reach the parent as WM_PARENTNOTIFY (see `watch_child_clicks`).
        let ex = GetWindowLongPtrW(hwnd(child), GWL_EXSTYLE);
        SetWindowLongPtrW(hwnd(child), GWL_EXSTYLE, ex & !(WS_EX_NOPARENTNOTIFY.0 as isize));
        let set = SetParent(hwnd(child), hwnd(parent));
        if GetParent(hwnd(child)).map(|p| p.0 as isize).ok() != Some(parent) {
            SetWindowLongPtrW(hwnd(child), GWL_STYLE, old_style);
            let reason = set.err().map_or_else(|| "the parent did not change".to_string(), |e| e.message());
            return Err(format!("SetParent failed: {reason}"));
        }
        // The host paints around its children, and its other children (the web view) leave the
        // embedded window's area alone.
        let parent_style = GetWindowLongPtrW(hwnd(parent), GWL_STYLE);
        SetWindowLongPtrW(hwnd(parent), GWL_STYLE, parent_style | WS_CLIPCHILDREN.0 as isize);
        for sibling in children_of(parent) {
            if sibling != child {
                let s = GetWindowLongPtrW(hwnd(sibling), GWL_STYLE);
                SetWindowLongPtrW(hwnd(sibling), GWL_STYLE, s | WS_CLIPSIBLINGS.0 as isize);
            }
        }
        Ok(old_style)
    }
}

/// [`embed`]s `child` in `parent` and puts it straight at `at` (client pixels), before it can be
/// seen anywhere else in it (out of sight, it goes in out of sight). Waits for `child`'s thread,
/// as `embed` does: not on the app's main thread.
pub fn embed_at(child: isize, parent: isize, at: Area) -> Result<isize, String> {
    let old_style = embed(child, parent)?;
    move_child(child, at, false);
    Ok(old_style)
}

/// The direct children of a window.
fn children_of(parent: isize) -> Vec<isize> {
    use windows::Win32::UI::WindowsAndMessaging::{EnumChildWindows, GetParent};
    unsafe extern "system" fn collect(window: HWND, out: LPARAM) -> BOOL {
        let list = &mut *(out.0 as *mut Vec<isize>);
        list.push(window.0 as isize);
        BOOL(1)
    }
    let mut all: Vec<isize> = Vec::new();
    unsafe {
        let _ = EnumChildWindows(hwnd(parent), Some(collect), LPARAM(&mut all as *mut _ as isize));
    }
    all.into_iter()
        .filter(|w| unsafe { GetParent(hwnd(*w)) }.is_ok_and(|p| p.0 as isize == parent))
        .collect()
}

/// Gives an embedded window back its own frame, as a top-level window again. Waits for its thread,
/// as [`embed`] does: not on the app's main thread.
pub fn unembed(child: isize, old_style: isize) {
    use windows::Win32::UI::WindowsAndMessaging::{
        SetParent, SetWindowLongPtrW, GWL_STYLE, SWP_FRAMECHANGED, SWP_NOMOVE, SWP_NOSIZE,
    };
    if !is_window(child) {
        return;
    }
    unsafe {
        let _ = SetParent(hwnd(child), None);
        SetWindowLongPtrW(hwnd(child), GWL_STYLE, old_style);
        let _ = SetWindowPos(hwnd(child), None, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED);
    }
}

/// The window Tauri lays over a frameless window's whole client area to catch its resize edges
/// (tauri-runtime-wry `undecorated_resizing.rs`; its window region is only the edge ring). Tauri
/// raises it to the top of its siblings on every resize.
const RESIZE_BORDERS_CLASS: &str = "TAURI_DRAG_RESIZE_BORDERS";

fn resize_borders_of(parent: isize) -> Option<isize> {
    use windows::Win32::UI::WindowsAndMessaging::FindWindowExW;
    let class: Vec<u16> = RESIZE_BORDERS_CLASS.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe { FindWindowExW(hwnd(parent), None, PCWSTR(class.as_ptr()), PCWSTR::null()) }
        .ok()
        .map(|w| w.0 as isize)
        .filter(|w| *w != 0)
}

/// Shows or hides Tauri's resize edges on a frameless window (hidden while it fills a monitor, so
/// the edges of the screen belong to what is in it). Tauri only moves them, so hidden stays hidden.
pub fn set_resize_borders(parent: isize, shown: bool) {
    let Some(borders) = resize_borders_of(parent) else { return };
    if is_visible(borders) != shown {
        if shown {
            show_without_focus(borders);
        } else {
            hide(borders);
        }
    }
}

/// Places an embedded window inside its parent's client area (`r` in client pixels): above its
/// siblings (the web view), but under Tauri's resize edges while they show. Never waits for the
/// viewer: the viewer's thread may be busy (rendering a frame), and once its window is embedded,
/// that thread and the parent's share their input, so `SWP_ASYNCWINDOWPOS` from the parent's
/// thread waits for it like a plain call (the app stopped responding there, seen live with the
/// display full screen). The request goes from a thread of its own, which shares input with no
/// one, so Windows hands it to the viewer's thread to do when it gets to it. In order; when
/// several are waiting, only the latest for each window is made.
pub fn place_child(child: isize, r: Area) {
    use std::sync::mpsc::{channel, Sender};
    static PLACER: std::sync::OnceLock<Sender<(isize, Area)>> = std::sync::OnceLock::new();
    let placer = PLACER.get_or_init(|| {
        let (tx, rx) = channel::<(isize, Area)>();
        let _ = std::thread::Builder::new().name("expand-place-viewer".into()).spawn(move || {
            while let Ok(first) = rx.recv() {
                let mut latest = vec![first];
                while let Ok((child, r)) = rx.try_recv() {
                    latest.retain(|(c, _)| *c != child);
                    latest.push((child, r));
                }
                for (child, r) in latest {
                    move_child(child, r, true);
                }
            }
        });
        tx
    });
    let _ = placer.send((child, r));
}

/// Places an embedded window at `r` in its parent: above its siblings, under the resize edges
/// while they show. `ask`: with `SWP_ASYNCWINDOWPOS`, for [`place_child`]'s thread, which shares
/// input with no other, so it returns at once; else it waits for the window's thread.
fn move_child(child: isize, r: Area, ask: bool) {
    use windows::Win32::UI::WindowsAndMessaging::{GetParent, HWND_TOP, SWP_ASYNCWINDOWPOS, SWP_FRAMECHANGED};
    if !is_window(child) {
        return;
    }
    let (x, y) = (r.left.round() as i32, r.top.round() as i32);
    let (w, h) = (((r.right - r.left).round() as i32).max(0), ((r.bottom - r.top).round() as i32).max(0));
    unsafe {
        let parent = GetParent(hwnd(child)).map(|p| p.0 as isize).unwrap_or(0);
        let after = resize_borders_of(parent).filter(|b| is_visible(*b)).map_or(HWND_TOP, hwnd);
        let flags = SWP_NOACTIVATE | SWP_FRAMECHANGED | if ask { SWP_ASYNCWINDOWPOS } else { Default::default() };
        let _ = SetWindowPos(hwnd(child), after, x, y, w, h, flags);
    }
}

pub fn is_minimized(window: isize) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::IsIconic;
    unsafe { IsIconic(hwnd(window)).as_bool() }
}

/// Who hears that a child window of a watched window is being destroyed ([`watch_child_clicks`]).
static CHILD_GOING: std::sync::OnceLock<fn(isize)> = std::sync::OnceLock::new();

/// Calls `hook` with a child of a window watched by [`watch_child_clicks`] as soon as that child
/// is being destroyed (the viewer's window, the moment it closes: it is still there then). Any
/// child: the hook tells which it is. Set once; on the watched window's thread when it runs.
pub fn on_child_going(hook: fn(isize)) {
    let _ = CHILD_GOING.set(hook);
}

/// Gives the keyboard to the embedded viewer window whenever it is clicked. `parent` must be a
/// window of the calling thread (Windows only allows subclassing on its own thread). A click on a
/// child reaches its parent as WM_PARENTNOTIFY whether or not the parent was already active, so
/// this also covers a click on the stream after using the title bar (the window is active then,
/// so no focus event comes). A child being destroyed is told to [`on_child_going`]'s hook.
pub fn watch_child_clicks(parent: isize) {
    use windows::Win32::Foundation::LRESULT;
    use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetParent, WM_DESTROY, WM_LBUTTONDOWN, WM_MBUTTONDOWN, WM_PARENTNOTIFY, WM_RBUTTONDOWN, WM_XBUTTONDOWN,
    };
    unsafe extern "system" fn proc(window: HWND, msg: u32, w: WPARAM, l: LPARAM, _: usize, _: usize) -> LRESULT {
        if msg == WM_PARENTNOTIFY && w.0 as u32 & 0xFFFF == WM_DESTROY {
            // The child going is in `l` (`embed` clears WS_EX_NOPARENTNOTIFY on the viewer's).
            if let Some(hook) = CHILD_GOING.get() {
                hook(l.0);
            }
        } else if msg == WM_PARENTNOTIFY
            && matches!(w.0 as u32 & 0xFFFF, WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN)
        {
            let child = EMBEDDED_VIEWER.load(std::sync::atomic::Ordering::Relaxed);
            let parent = window.0 as isize;
            let embedded_here =
                is_window(child) && GetParent(hwnd(child)).is_ok_and(|p| p.0 as isize == parent);
            // The point is in the parent's client positions.
            let mut at = POINT { x: (l.0 & 0xFFFF) as i16 as i32, y: ((l.0 >> 16) & 0xFFFF) as i16 as i32 };
            let _ = ClientToScreen(window, &mut at);
            let on_child = window_rect(child).is_some_and(|r| r.contains((at.x as f64, at.y as f64)));
            if embedded_here && on_child && !(w.0 as u32 & 0xFFFF == WM_LBUTTONDOWN && focus_while_sent(child)) {
                focus(child, move || root_in_front(child));
            }
        }
        DefSubclassProc(window, msg, w, l)
    }
    unsafe {
        let _ = SetWindowSubclass(hwnd(parent), Some(proc), 0x5353, 0);
    }
}

/// A left click on the embedded window that lacks the keyboard (the title bar was used last):
/// gives it the keyboard right now, from inside the click's `WM_PARENTNOTIFY`, before the viewer
/// reads the click. The viewer only takes a click after it has the keyboard (it drops the one
/// that gives it the keyboard: the first click on the stream did nothing). Done only while the
/// viewer's thread is waiting for this very message (it sends it as it reads the click), so it
/// handles the focus messages `SetFocus` sends it at once: nothing here waits on a busy viewer.
/// Whether the keyboard is there now; when not, the caller asks [`focus`] as before.
fn focus_while_sent(child: isize) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetFocus, SetFocus};
    use windows::Win32::UI::WindowsAndMessaging::{InSendMessageEx, ISMEX_NOTIFY, ISMEX_REPLIED, ISMEX_SEND};
    unsafe {
        let how = InSendMessageEx(None) & (ISMEX_SEND | ISMEX_NOTIFY | ISMEX_REPLIED);
        if how != ISMEX_SEND {
            return false;
        }
        if GetFocus().0 as isize == child {
            return true;
        }
        let _ = SetFocus(hwnd(child));
        GetFocus().0 as isize == child
    }
}

/// Whether the top-level window `window` is in (itself, or the one it is embedded in) is the
/// window in front.
pub fn root_in_front(window: isize) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{GetAncestor, GA_ROOT};
    let root = unsafe { GetAncestor(hwnd(window), GA_ROOT) }.0 as isize;
    root != 0 && root == foreground_window()
}

/// Gives keyboard input to an embedded window, from any thread, and never waits for it. Moving the
/// keyboard sends the window that has it and the one that gets it a message each and waits for
/// their threads, and the embedded window's thread (the viewer's) may be busy: done on the thread
/// the window is embedded in (the app's main thread), it waited as long as the viewer's thread
/// was busy. So it is done on a thread of its own, which joins the input the two threads share
/// (`SetFocus` only works there) just for the call. In order; when several are waiting, only the latest for each window is done,
/// and only if its `wanted` still says so then (it may come late: the keyboard may have gone
/// elsewhere meanwhile). Nothing is done while the window has the keyboard already.
pub fn focus(child: isize, wanted: impl Fn() -> bool + Send + 'static) {
    ask_keyboard_thread(child, Ask::Keyboard, Box::new(wanted));
}

/// Makes `root` (the window `child` is embedded in) the window in front, then gives `child` the
/// keyboard, from the same thread as [`focus`] and never waiting for anyone, if `wanted` still
/// says so when that thread gets to it. The window in front may be another process's (the
/// viewer's, as it starts), which Windows may refuse to give the front up for: that thread joins
/// the input of the window in front for the call ([`bring_forward`]). A refusal is logged.
pub fn take_front(child: isize, root: isize, wanted: impl Fn() -> bool + Send + 'static) {
    ask_keyboard_thread(child, Ask::Front(root), Box::new(wanted));
}

/// What the keyboard thread is asked to do for an embedded window.
#[derive(Clone, Copy, PartialEq)]
enum Ask {
    Keyboard,
    /// The window it is in comes in front first.
    Front(isize),
}

fn ask_keyboard_thread(child: isize, ask: Ask, wanted: Box<dyn Fn() -> bool + Send>) {
    use std::sync::mpsc::{channel, Sender};
    type Request = (isize, Ask, Box<dyn Fn() -> bool + Send>);
    static KEYBOARD: std::sync::OnceLock<Sender<Request>> = std::sync::OnceLock::new();
    let keyboard = KEYBOARD.get_or_init(|| {
        let (tx, rx) = channel::<Request>();
        let _ = std::thread::Builder::new().name("expand-focus-viewer".into()).spawn(move || {
            // AttachThreadInput needs a message queue on this thread.
            let mut msg = windows::Win32::UI::WindowsAndMessaging::MSG::default();
            unsafe {
                use windows::Win32::UI::WindowsAndMessaging::{PeekMessageW, PM_NOREMOVE};
                let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
            }
            while let Ok(first) = rx.recv() {
                let mut latest = vec![first];
                while let Ok((child, ask, wanted)) = rx.try_recv() {
                    latest.retain(|(c, a, _)| (*c, *a) != (child, ask));
                    latest.push((child, ask, wanted));
                }
                for (child, ask, wanted) in latest {
                    match ask {
                        Ask::Keyboard => {
                            if is_window(child) && keyboard_focus(hwnd(child)) != child && wanted() {
                                give_keyboard(child);
                            }
                        }
                        Ask::Front(root) => {
                            if !is_window(child) || !wanted() {
                                continue;
                            }
                            if foreground_window() != root && !bring_forward(root) {
                                eprintln!(
                                    "expand: Windows refused the display window the front (in front: {})",
                                    describe(foreground_window())
                                );
                                continue;
                            }
                            if keyboard_focus(hwnd(child)) != child {
                                give_keyboard(child);
                            }
                        }
                    }
                }
            }
        });
        tx
    });
    let _ = keyboard.send((child, ask, wanted));
}

/// `SetForegroundWindow(window)` for [`take_front`], on the keyboard thread. Windows lets a
/// thread take the front from the window in front when it shares that window's input, so this
/// thread joins the input of the window in front's thread just for the call (that thread and
/// the window's own may be sent activation messages and waited for: never the main thread).
/// Whether `window` is in front after it.
fn bring_forward(window: isize) -> bool {
    use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    unsafe {
        let front = GetForegroundWindow();
        let (theirs, ours) = (GetWindowThreadProcessId(front, None), GetCurrentThreadId());
        let joined = theirs != 0 && theirs != ours && AttachThreadInput(ours, theirs, true).as_bool();
        let _ = SetForegroundWindow(hwnd(window));
        if joined {
            let _ = AttachThreadInput(ours, theirs, false);
        }
    }
    foreground_window() == window
}

/// `SetFocus` for [`focus`], on its thread: joined to the input of `child`'s thread for the call.
fn give_keyboard(child: isize) {
    use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
    unsafe {
        let (theirs, ours) = (GetWindowThreadProcessId(hwnd(child), None), GetCurrentThreadId());
        if theirs == 0 || theirs == ours || !AttachThreadInput(ours, theirs, true).as_bool() {
            return;
        }
        let _ = SetFocus(hwnd(child));
        let _ = AttachThreadInput(ours, theirs, false);
    }
}

/// The scale of the monitor a window is on (1.0 at 96 dpi).
pub fn scale_of_window(window: isize) -> f64 {
    use windows::Win32::UI::HiDpi::GetDpiForWindow;
    match unsafe { GetDpiForWindow(hwnd(window)) } {
        0 => 1.0,
        dpi => dpi as f64 / 96.0,
    }
}

/// The scale of the monitor `m` (1.0 at 96 dpi).
pub fn scale_of_monitor(m: Area) -> f64 {
    use windows::Win32::Graphics::Gdi::{MonitorFromPoint, MONITOR_DEFAULTTONEAREST};
    use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
    let (mut x, mut y) = (0u32, 0u32);
    let centre = POINT { x: ((m.left + m.right) / 2.0) as i32, y: ((m.top + m.bottom) / 2.0) as i32 };
    unsafe {
        let monitor = MonitorFromPoint(centre, MONITOR_DEFAULTTONEAREST);
        if GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut x, &mut y).is_err() || x == 0 {
            return 1.0;
        }
    }
    x as f64 / 96.0
}

/// Limits a window to `region` (pixels from its top-left corner): only there is it drawn and
/// does it take the pointer; elsewhere clicks reach what is under it. `None`: nowhere (an empty
/// region; no region at all would be the whole window). From the window's own thread.
pub fn set_region(window: isize, region: Option<Area>) {
    use windows::Win32::Graphics::Gdi::{CreateRectRgn, SetWindowRgn};
    let r = region.unwrap_or(Area { left: 0.0, top: 0.0, right: 0.0, bottom: 0.0 });
    unsafe {
        let rgn = CreateRectRgn(r.left.round() as i32, r.top.round() as i32, r.right.round() as i32, r.bottom.round() as i32);
        // Windows owns the region from here on.
        SetWindowRgn(hwnd(window), rgn, true);
    }
}

/// Moves and sizes one of this app's windows to exactly `to`. Moving it onto a monitor with
/// another scale makes Windows resize it for that scale, so the size is set again after.
pub fn place_window(window: isize, to: Area) {
    move_window(window, to);
    if window_rect(window) != Some(to) {
        move_window(window, to);
    }
}

/// Whether a window is maximized.
pub fn is_zoomed(window: isize) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::IsZoomed;
    unsafe { IsZoomed(hwnd(window)).as_bool() }
}

/// How far the work area's corner of the monitor `r` is from the monitor's own corner. A window
/// placement (`GetWindowPlacement`) is in workspace positions: screen positions less this.
fn workspace_offset(r: RECT) -> (i32, i32) {
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromRect, MONITORINFO, MONITOR_DEFAULTTONEAREST};
    let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    unsafe {
        if !GetMonitorInfoW(MonitorFromRect(&r, MONITOR_DEFAULTTONEAREST), &mut info).as_bool() {
            return (0, 0);
        }
    }
    (info.rcWork.left - info.rcMonitor.left, info.rcWork.top - info.rcMonitor.top)
}

fn placement_of(window: isize) -> Option<windows::Win32::UI::WindowsAndMessaging::WINDOWPLACEMENT> {
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowPlacement, WINDOWPLACEMENT};
    let mut p = WINDOWPLACEMENT { length: std::mem::size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
    unsafe { GetWindowPlacement(hwnd(window), &mut p).ok()? };
    Some(p)
}

/// Where a maximized or minimized window goes back to (its restored place), in screen positions.
pub fn normal_rect(window: isize) -> Option<Area> {
    let r = placement_of(window)?.rcNormalPosition;
    let (dx, dy) = workspace_offset(r);
    Some(to_area(RECT { left: r.left + dx, top: r.top + dy, right: r.right + dx, bottom: r.bottom + dy }))
}

/// Takes a minimized or maximized window straight to `to`, shown, without making it the window
/// in front: it never shows at its old place first (as `SW_RESTORE` would). Follow it with
/// [`place_window`], since the placement is only as exact as Windows' workspace positions.
pub fn restore_to(window: isize, to: Area) {
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNOACTIVATE;
    set_placement(window, to, SW_SHOWNOACTIVATE.0 as u32);
}

/// Sets where a minimized window comes back to when it is restored (`to`), and leaves it
/// minimized: moving a minimized window would only move its icon.
pub fn set_restored_place(window: isize, to: Area) {
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWMINNOACTIVE;
    set_placement(window, to, SW_SHOWMINNOACTIVE.0 as u32);
}

/// `SetWindowPlacement` with the restored place `to` (screen positions) and `show`.
fn set_placement(window: isize, to: Area, show: u32) {
    use windows::Win32::UI::WindowsAndMessaging::{SetWindowPlacement, WINDOWPLACEMENT_FLAGS};
    let Some(mut p) = placement_of(window) else { return };
    let r = RECT { left: to.left.round() as i32, top: to.top.round() as i32, right: to.right.round() as i32, bottom: to.bottom.round() as i32 };
    let (dx, dy) = workspace_offset(r);
    p.rcNormalPosition = RECT { left: r.left - dx, top: r.top - dy, right: r.right - dx, bottom: r.bottom - dy };
    p.showCmd = show;
    // Not "restore to maximized" (a window minimized while maximized would come back maximized).
    p.flags = WINDOWPLACEMENT_FLAGS(0);
    unsafe {
        let _ = SetWindowPlacement(hwnd(window), &p);
    }
}

/// The window that fills a monitor (0 for none). Its whole rect is its client area: Tauri's
/// frameless windows otherwise keep an invisible resize frame on three sides (with the shadow),
/// which would leave strips of the desktop showing at the monitor's edges.
static FILLING: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

/// Answers WM_NCCALCSIZE for `parent`: while it fills a monitor ([`set_filling`]), no frame at
/// all; as a window (not maximized), no frame above its client area, so its title bar reaches
/// the very top edge. Tao keeps a strip there on Windows 11 (1 px at 125%), which Windows draws
/// as a grey line above the title bar; with no border line ([`set_filling`]) nothing covers the
/// top row instead. The sides and bottom keep their invisible resize frame. Must be called on
/// `parent`'s thread (subclassing), after Tauri made the window, so this runs before Tauri's own
/// frame handling.
pub fn watch_frame(parent: isize) {
    use windows::Win32::Foundation::LRESULT;
    use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
    use windows::Win32::UI::WindowsAndMessaging::{NCCALCSIZE_PARAMS, WM_NCCALCSIZE};
    unsafe extern "system" fn proc(window: HWND, msg: u32, w: WPARAM, l: LPARAM, _: usize, _: usize) -> LRESULT {
        if msg == WM_NCCALCSIZE {
            if FILLING.load(std::sync::atomic::Ordering::Relaxed) == window.0 as isize {
                // The proposed window rect stays as it is: all of it is client area.
                return LRESULT(0);
            }
            if !is_zoomed(window.0 as isize) {
                // The first rect is the proposed window rect in, the client rect out.
                let rect = if w.0 == 0 { l.0 as *mut RECT } else { (*(l.0 as *mut NCCALCSIZE_PARAMS)).rgrc.as_mut_ptr() };
                let top = (*rect).top;
                let result = DefSubclassProc(window, msg, w, l);
                (*rect).top = top;
                return result;
            }
        }
        DefSubclassProc(window, msg, w, l)
    }
    unsafe {
        let _ = SetWindowSubclass(hwnd(parent), Some(proc), 0x5354, 0);
    }
}

/// `DWMWA_BORDER_COLOR`: no border line.
const DWM_COLOR_NONE: u32 = 0xFFFF_FFFE;

/// Makes a window watched by [`watch_frame`] fill its rect edge to edge with square corners and
/// no border line (`true`), or gives it its frame back; with `to`, moved and sized there in the
/// same step. With no frame there is no shadow either (Tauri's frameless shadow lives in that
/// frame). One step, so nothing shows half changed (the frame gone but the window still small,
/// or the frame back round a window still the size of its monitor). Through Windows only, so
/// from any thread.
pub fn set_filling(window: isize, filling: bool, to: Option<Area>) {
    use std::sync::atomic::Ordering;
    use windows::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DEFAULT, DWMWCP_DONOTROUND,
    };
    use windows::Win32::UI::WindowsAndMessaging::{SWP_FRAMECHANGED, SWP_NOMOVE, SWP_NOSIZE};
    if filling {
        FILLING.store(window, Ordering::Relaxed);
    } else {
        let _ = FILLING.compare_exchange(window, 0, Ordering::Relaxed, Ordering::Relaxed);
    }
    let corners = if filling { DWMWCP_DONOTROUND } else { DWMWCP_DEFAULT };
    // None as a window either: its client area reaches its top edge ([`watch_frame`]), where
    // the border line would be a grey line over the title bar.
    let border = DWM_COLOR_NONE;
    let flags = SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED;
    unsafe {
        // Round corners would show the desktop at the monitor's corners, and Windows 11's border
        // line would show round the monitor's edges (lighter as the window comes in front or
        // goes). Windows 11 only; Windows 10 refuses them, and has neither.
        let _ = DwmSetWindowAttribute(hwnd(window), DWMWA_WINDOW_CORNER_PREFERENCE, &corners as *const _ as _, 4);
        let _ = DwmSetWindowAttribute(hwnd(window), DWMWA_BORDER_COLOR, &border as *const _ as _, 4);
        // The frame is worked out again (WM_NCCALCSIZE), in the same step as the move.
        match to {
            Some(to) => {
                let (x, y) = (to.left.round() as i32, to.top.round() as i32);
                let (w, h) = ((to.right - to.left).round() as i32, (to.bottom - to.top).round() as i32);
                let _ = SetWindowPos(hwnd(window), None, x, y, w, h, flags);
                // Onto a monitor with another scale, Windows sizes it for that scale.
                if window_rect(window) != Some(to) {
                    move_window(window, to);
                }
            }
            None => {
                let _ = SetWindowPos(hwnd(window), None, 0, 0, 0, 0, flags | SWP_NOMOVE | SWP_NOSIZE);
            }
        }
    }
    crate::trace::line(|| format!("host {window:#x} {} {to:?}", if filling { "fills" } else { "framed" }));
}

/// An always-on-top overlay of this app's (the bar): no show or hide animation, square corners,
/// no border line. Windows 11 only; Windows 10 refuses the last two, and has neither.
pub fn plain_overlay(window: isize) {
    use windows::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_TRANSITIONS_FORCEDISABLED, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND,
    };
    let off: BOOL = true.into();
    unsafe {
        let _ = DwmSetWindowAttribute(hwnd(window), DWMWA_TRANSITIONS_FORCEDISABLED, &off as *const _ as _, 4);
        let _ = DwmSetWindowAttribute(hwnd(window), DWMWA_WINDOW_CORNER_PREFERENCE, &DWMWCP_DONOTROUND as *const _ as _, 4);
        let _ = DwmSetWindowAttribute(hwnd(window), DWMWA_BORDER_COLOR, &DWM_COLOR_NONE as *const _ as _, 4);
    }
    no_frame_painting(window);
}

/// Windows never paints a frame on `window`: as it became the window in front (the bar taking the
/// keyboard for a menu), Windows drew a whole white title bar ("Expand bar", with its
/// buttons) over it for a moment, seen live as a white bar behind the bar (round 4). Activation
/// itself goes on as before. Must be called on `window`'s thread (subclassing).
pub fn no_frame_painting(window: isize) {
    use windows::Win32::Foundation::LRESULT;
    use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
    use windows::Win32::UI::WindowsAndMessaging::{WM_NCACTIVATE, WM_NCPAINT};
    unsafe extern "system" fn proc(window: HWND, msg: u32, w: WPARAM, l: LPARAM, _: usize, _: usize) -> LRESULT {
        match msg {
            // -1: the activation goes through without the frame being repainted for it.
            WM_NCACTIVATE => DefSubclassProc(window, msg, w, LPARAM(-1)),
            WM_NCPAINT => LRESULT(0),
            _ => DefSubclassProc(window, msg, w, l),
        }
    }
    unsafe {
        let _ = SetWindowSubclass(hwnd(window), Some(proc), 0x5355, 0);
    }
}

thread_local! {
    static TASKBAR: std::cell::RefCell<Option<windows::Win32::UI::Shell::ITaskbarList2>> = const { std::cell::RefCell::new(None) };
}

/// Tells the taskbar that a window is full screen (it then goes behind the window while that is
/// in front, on its monitor), or no longer. Needs COM: call it on the app's main thread.
pub fn mark_full_screen(window: isize, full_screen: bool) {
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
    use windows::Win32::UI::Shell::{ITaskbarList2, TaskbarList};
    TASKBAR.with(|cell| {
        let mut list = cell.borrow_mut();
        if list.is_none() {
            let made: Option<ITaskbarList2> = unsafe { CoCreateInstance(&TaskbarList, None, CLSCTX_INPROC_SERVER) }.ok();
            *list = made.filter(|l| unsafe { l.HrInit() }.is_ok());
        }
        if let Some(list) = list.as_ref() {
            let _ = unsafe { list.MarkFullscreenWindow(hwnd(window), BOOL::from(full_screen)) };
        }
    });
}

/// The part of monitor `m` that windows go in (without the taskbar).
pub fn work_area(m: Area) -> Area {
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST};
    let centre = POINT { x: ((m.left + m.right) / 2.0) as i32, y: ((m.top + m.bottom) / 2.0) as i32 };
    let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    unsafe {
        if !GetMonitorInfoW(MonitorFromPoint(centre, MONITOR_DEFAULTTONEAREST), &mut info).as_bool() {
            return m;
        }
    }
    to_area(info.rcWork)
}

/// Whether the primary mouse button is held down right now. The physical button: with the
/// buttons swapped, that is the right one.
pub fn primary_button_down() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON};
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_SWAPBUTTON};
    unsafe {
        let button = if GetSystemMetrics(SM_SWAPBUTTON) != 0 { VK_RBUTTON } else { VK_LBUTTON };
        GetAsyncKeyState(button.0 as i32) < 0
    }
}

/// Moves `window` with the pointer until the button is let go: Windows' own move, as from a title
/// bar, so Aero Snap works too. Only while the primary button is still down (started with it up,
/// the window would follow the pointer until the next click) and the window is in front (the
/// button is then this app's); returns whether it moved. Whoever holds the mouse (the web view the
/// button went down in) lets go of it first. On the window's thread, at the top of its message
/// loop: it returns when the move ends.
pub fn begin_move(window: isize) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture;
    use windows::Win32::UI::WindowsAndMessaging::{HTCAPTION, SC_MOVE, WM_SYSCOMMAND};
    if !primary_button_down() || foreground_window() != window {
        return false;
    }
    unsafe {
        let _ = ReleaseCapture();
        SendMessageW(hwnd(window), WM_SYSCOMMAND, WPARAM((SC_MOVE | HTCAPTION) as usize), LPARAM(0));
    }
    true
}

/// The message [`begin_move_later`] posts.
fn move_message() -> u32 {
    use windows::core::w;
    use windows::Win32::UI::WindowsAndMessaging::RegisterWindowMessageW;
    static MESSAGE: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *MESSAGE.get_or_init(|| unsafe { RegisterWindowMessageW(w!("Expand.BeginMove")) })
}

/// Asks `window` (watched by [`watch_moves`]) to [`begin_move`] once its thread is back at its
/// message loop: not inside the caller, which may be a web view's callback (a move loop there
/// would hold that callback, and everything waiting on it, until the button is let go). The
/// button is checked then. From any thread.
pub fn begin_move_later(window: isize) {
    use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
    unsafe {
        let _ = PostMessageW(hwnd(window), move_message(), WPARAM(0), LPARAM(0));
    }
}

/// A move or size of a window watched by [`watch_moves`] is under way (its modal loop runs).
static MOVING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// The window's scale changed during that move (Windows sized it for the new monitor).
static SCALED_WHILE_MOVING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Who hears that a move ended ([`on_move_end`]).
static MOVE_END: std::sync::OnceLock<fn(isize, bool)> = std::sync::OnceLock::new();

/// Calls `hook` with a window watched by [`watch_moves`] when the user ends moving or sizing it,
/// and whether its scale changed on the way. Set once; on the window's thread, inside its loop.
pub fn on_move_end(hook: fn(isize, bool)) {
    let _ = MOVE_END.set(hook);
}

/// Whether the user is moving or sizing a window watched by [`watch_moves`] right now.
pub fn in_move_loop() -> bool {
    MOVING.load(std::sync::atomic::Ordering::Relaxed)
}

/// Lets [`begin_move_later`] reach `parent`, and tells [`on_move_end`]'s hook when a move ends.
/// On `parent`'s thread (subclassing), after Tauri made the window.
pub fn watch_moves(parent: isize) {
    use std::sync::atomic::Ordering;
    use windows::Win32::Foundation::LRESULT;
    use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
    use windows::Win32::UI::WindowsAndMessaging::{WM_DPICHANGED, WM_ENTERSIZEMOVE, WM_EXITSIZEMOVE};
    unsafe extern "system" fn proc(window: HWND, msg: u32, w: WPARAM, l: LPARAM, _: usize, _: usize) -> LRESULT {
        if msg == WM_ENTERSIZEMOVE {
            MOVING.store(true, Ordering::Relaxed);
            SCALED_WHILE_MOVING.store(false, Ordering::Relaxed);
        } else if msg == WM_DPICHANGED && MOVING.load(Ordering::Relaxed) {
            SCALED_WHILE_MOVING.store(true, Ordering::Relaxed);
        } else if msg == WM_EXITSIZEMOVE {
            MOVING.store(false, Ordering::Relaxed);
            let scaled = SCALED_WHILE_MOVING.swap(false, Ordering::Relaxed);
            if let Some(hook) = MOVE_END.get() {
                hook(window.0 as isize, scaled);
            }
        }
        if msg == move_message() {
            if !begin_move(window.0 as isize) {
                eprintln!("expand: the button was up (or the display window not in front) before it could follow the pointer; it stays where it is");
            }
            return LRESULT(0);
        }
        DefSubclassProc(window, msg, w, l)
    }
    let _ = move_message();
    unsafe {
        let _ = SetWindowSubclass(hwnd(parent), Some(proc), 0x5355, 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use windows::core::w;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE, WINDOW_EX_STYLE,
        WS_OVERLAPPEDWINDOW,
    };

    /// A run of the viewer that ends late (a pairing check given up after the stream started)
    /// never forgets the stream viewer's process; the one it recorded goes with it.
    #[test]
    fn only_the_process_recorded_is_forgotten() {
        set_viewer_process(Some(5));
        forget_viewer_process(7);
        let kept = viewer_process();
        forget_viewer_process(5);
        let gone = viewer_process();
        assert_eq!(kept, Some(5));
        assert_eq!(gone, None);
    }

    /// A hidden window (no WS_VISIBLE): nothing shows on screen.
    fn new_window() -> isize {
        let made = unsafe {
            CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), w!(""), WS_OVERLAPPEDWINDOW, 100, 100, 640, 480, None, None, None, None)
        };
        made.expect("a test window").0 as isize
    }

    fn pump() {
        let mut msg = MSG::default();
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    /// Pumps this thread's messages until `done` or 5 s; whether it got there.
    fn pump_until(done: impl Fn() -> bool) -> bool {
        let until = Instant::now() + Duration::from_secs(5);
        while !done() && Instant::now() < until {
            pump();
            std::thread::sleep(Duration::from_millis(10));
        }
        done()
    }

    /// The window with the keyboard on this thread's input (shared with an embedded window's).
    fn keyboard_here() -> isize {
        unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetFocus().0 as isize }
    }

    /// A viewer's window, made by a thread of its own (its message loop stalls for 1.5 s on
    /// [`Viewer::stall`], as a viewer busy with a frame), embedded in a window of this thread.
    struct Viewer {
        parent: isize,
        child: isize,
        busy: Arc<AtomicBool>,
        done: Arc<AtomicBool>,
        thread: std::thread::JoinHandle<()>,
    }

    impl Viewer {
        fn embedded() -> Viewer {
            let (tx, rx) = std::sync::mpsc::channel();
            let busy = Arc::new(AtomicBool::new(false));
            let done = Arc::new(AtomicBool::new(false));
            let (b, d) = (busy.clone(), done.clone());
            let thread = std::thread::spawn(move || {
                let child = new_window();
                tx.send(child).unwrap();
                while !d.load(Ordering::Relaxed) {
                    if b.swap(false, Ordering::Relaxed) {
                        std::thread::sleep(Duration::from_millis(1500));
                    }
                    pump();
                    std::thread::sleep(Duration::from_millis(5));
                }
                unsafe {
                    let _ = DestroyWindow(hwnd(child));
                }
            });
            let parent = new_window();
            let child = rx.recv().unwrap();
            embed(child, parent).expect("embedded");
            Viewer { parent, child, busy, done, thread }
        }

        /// The viewer's thread stops answering for 1.5 s, starting now.
        fn stall(&self) {
            self.busy.store(true, Ordering::Relaxed);
            std::thread::sleep(Duration::from_millis(100));
        }

        fn end(self) {
            self.done.store(true, Ordering::Relaxed);
            // The viewer's window tells its parent (this thread) as it goes.
            while !self.thread.is_finished() {
                pump();
                std::thread::sleep(Duration::from_millis(5));
            }
            self.thread.join().unwrap();
            unsafe {
                let _ = DestroyWindow(hwnd(self.parent));
            }
        }
    }

    /// The viewer's thread can be busy (Moonlight rendering a frame) when its window is placed:
    /// placing it must never wait for that thread, though the two threads share their input once
    /// it is embedded (which makes Windows ignore `SWP_ASYNCWINDOWPOS` on the caller's thread).
    #[test]
    fn placing_an_embedded_window_never_waits_for_its_busy_thread() {
        let viewer = Viewer::embedded();
        let child = viewer.child;
        viewer.stall();
        let started = Instant::now();
        place_child(child, Area { left: 0.0, top: 40.0, right: 320.0, bottom: 220.0 });
        let took = started.elapsed();
        // It still gets there once the viewer's thread is back at its message loop.
        let size = |r: Area| (r.right - r.left, r.bottom - r.top);
        let placed = pump_until(|| window_rect(child).map(size) == Some((320.0, 180.0)));
        viewer.end();
        assert!(took < Duration::from_millis(500), "placing waited {took:?} for the viewer's thread");
        assert!(placed, "the window was never placed");
    }

    /// Giving the keyboard to the embedded window must not wait for the viewer's busy thread
    /// either (`SetFocus` there sends it a message and waits; seen taking the whole stall), and
    /// the keyboard still gets there.
    #[test]
    fn giving_an_embedded_window_the_keyboard_never_waits_for_its_busy_thread() {
        let viewer = Viewer::embedded();
        let child = viewer.child;
        viewer.stall();
        let started = Instant::now();
        focus(child, || true);
        let took = started.elapsed();
        let given = pump_until(|| keyboard_here() == child);
        viewer.end();
        assert!(took < Duration::from_millis(500), "giving the keyboard waited {took:?} for the viewer's thread");
        assert!(given, "the embedded window never got the keyboard");
    }

    /// A left click on the stream after the title bar was used: the stream has the keyboard by
    /// the time its thread reads the click (it sends its parent `WM_PARENTNOTIFY` first and waits
    /// for it), so the viewer takes that first click as a click.
    #[test]
    fn a_left_click_on_the_stream_gives_it_the_keyboard_before_the_click_is_read() {
        use windows::Win32::UI::WindowsAndMessaging::{WM_LBUTTONDOWN, WM_PARENTNOTIFY};
        let parent = new_window();
        watch_child_clicks(parent);
        let (tx, rx) = std::sync::mpsc::channel();
        let (go, went) = std::sync::mpsc::channel::<()>();
        let viewer = std::thread::spawn(move || {
            let child = new_window();
            tx.send(child).unwrap();
            // Answering meanwhile: the embed waits for this thread.
            while went.try_recv().is_err() {
                pump();
                std::thread::sleep(Duration::from_millis(5));
            }
            // As Windows does while the viewer's thread reads a click: the click goes to the
            // parent first, and the thread waits for it.
            let at = LPARAM(150 | (150 << 16));
            unsafe { SendMessageW(hwnd(parent), WM_PARENTNOTIFY, WPARAM(WM_LBUTTONDOWN as usize), at) };
            let had = keyboard_here() == child;
            unsafe {
                let _ = DestroyWindow(hwnd(child));
            }
            had
        });
        let child = rx.recv().unwrap();
        embed(child, parent).expect("embedded");
        set_embedded_viewer(Some(child));
        go.send(()).unwrap();
        while !viewer.is_finished() {
            pump();
            std::thread::sleep(Duration::from_millis(5));
        }
        let had = viewer.join().unwrap();
        set_embedded_viewer(None);
        unsafe {
            let _ = DestroyWindow(hwnd(parent));
        }
        assert!(had, "the stream did not have the keyboard when it read the click");
    }

    /// Outside a click sent by the viewer's thread, nothing is done here (the focus thread does
    /// it, never waiting on the viewer).
    #[test]
    fn the_keyboard_is_given_at_once_only_inside_a_sent_click() {
        let window = new_window();
        assert!(!focus_while_sent(window));
        unsafe {
            let _ = DestroyWindow(hwnd(window));
        }
    }

    /// A request that is no longer wanted by the time it would be done is dropped: the keyboard
    /// does not go to the embedded window.
    #[test]
    fn the_keyboard_is_not_given_once_no_longer_wanted() {
        let viewer = Viewer::embedded();
        let child = viewer.child;
        let asked = Arc::new(AtomicBool::new(false));
        let seen = asked.clone();
        focus(child, move || {
            seen.store(true, Ordering::Relaxed);
            false
        });
        let checked = pump_until(|| asked.load(Ordering::Relaxed));
        // Time for a wrong move to land.
        for _ in 0..20 {
            pump();
            std::thread::sleep(Duration::from_millis(10));
        }
        let keyboard = keyboard_here();
        viewer.end();
        assert!(checked, "the request was never looked at");
        assert_ne!(keyboard, child);
    }

    /// A stand-in for the viewer's stream window: a hidden top-level window of the stream's class
    /// (registered in this process), made and served by a thread of its own, as Moonlight's is.
    struct StreamWindow {
        window: isize,
        moves: std::sync::mpsc::Sender<Area>,
        /// Where the window was right after the viewer's own last move returned (before its
        /// thread handled any message).
        after_move: Arc<std::sync::Mutex<Option<Area>>>,
        done: Arc<AtomicBool>,
        thread: std::thread::JoinHandle<()>,
    }

    impl StreamWindow {
        fn make(at: Area) -> StreamWindow {
            StreamWindow::make_as(w!("SDL_app"), at, false)
        }

        /// A window of `class` at `at`, made by a thread of its own; `shown`: shown, but cloaked,
        /// see-through, a tool window and never activated, so nothing of it is seen on screen.
        fn make_as(class: PCWSTR, at: Area, shown: bool) -> StreamWindow {
            StreamWindow::make_ghost(class, at, shown, shown)
        }

        /// [`make_as`](StreamWindow::make_as); `ghost`: cloaked and see-through even while hidden,
        /// for a window the code under test may show.
        fn make_ghost(class: PCWSTR, at: Area, shown: bool, ghost: bool) -> StreamWindow {
            StreamWindow::make_full(class, at, shown, ghost).0
        }

        /// [`make_ghost`](StreamWindow::make_ghost), and where the window was the moment
        /// `CreateWindowEx` returned on its thread (before its thread handled any message).
        fn make_full(class: PCWSTR, at: Area, shown: bool, ghost: bool) -> (StreamWindow, Option<Area>) {
            use windows::Win32::Foundation::{COLORREF, LRESULT};
            use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_CLOAK};
            use windows::Win32::UI::WindowsAndMessaging::{
                DefWindowProcW, RegisterClassW, SetLayeredWindowAttributes, LWA_ALPHA, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
                WS_EX_TOOLWINDOW,
            };
            unsafe extern "system" fn proc(window: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
                DefWindowProcW(window, msg, w, l)
            }
            unsafe {
                // A second registration of the same class fails, and the first one stands.
                let wc = WNDCLASSW { lpfnWndProc: Some(proc), lpszClassName: class, ..Default::default() };
                RegisterClassW(&wc);
            }
            let class = class.0 as usize;
            let (tx, rx) = std::sync::mpsc::channel();
            let (moves, asked) = std::sync::mpsc::channel::<Area>();
            let after_move: Arc<std::sync::Mutex<Option<Area>>> = Default::default();
            let after = after_move.clone();
            let done = Arc::new(AtomicBool::new(false));
            let d = done.clone();
            let thread = std::thread::spawn(move || {
                let (x, y) = (at.left as i32, at.top as i32);
                let (cx, cy) = ((at.right - at.left) as i32, (at.bottom - at.top) as i32);
                let ex = if ghost { WS_EX_LAYERED | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW } else { WINDOW_EX_STYLE(0) };
                let class = PCWSTR(class as *const u16);
                let made = unsafe { CreateWindowExW(ex, class, w!("stream"), WS_OVERLAPPEDWINDOW, x, y, cx, cy, None, None, None, None) };
                let window = made.expect("a test window").0 as isize;
                let made_at = window_rect(window);
                if ghost {
                    unsafe {
                        let cloak: BOOL = true.into();
                        let _ = DwmSetWindowAttribute(hwnd(window), DWMWA_CLOAK, &cloak as *const _ as _, 4);
                        let _ = SetLayeredWindowAttributes(hwnd(window), COLORREF(0), 0, LWA_ALPHA);
                    }
                }
                if shown {
                    unsafe {
                        let _ = ShowWindow(hwnd(window), SW_SHOWNOACTIVATE);
                    }
                }
                tx.send((window, made_at)).unwrap();
                while !d.load(Ordering::Relaxed) {
                    // The viewer moves its own window.
                    if let Ok(to) = asked.try_recv() {
                        move_window(window, to);
                        *after.lock().unwrap() = window_rect(window);
                    }
                    pump();
                    std::thread::sleep(Duration::from_millis(5));
                }
                unsafe {
                    let _ = DestroyWindow(hwnd(window));
                }
            });
            let (window, made_at) = rx.recv().unwrap();
            (StreamWindow { window, moves, after_move, done, thread }, made_at)
        }

        fn end(self) {
            self.done.store(true, Ordering::Relaxed);
            // Embedded, the window tells its parent (this thread) as it goes.
            while !self.thread.is_finished() {
                pump();
                std::thread::sleep(Duration::from_millis(5));
            }
            self.thread.join().unwrap();
        }
    }

    /// Waits up to 3 s for `done`, without pumping (the windows involved are other threads').
    fn wait_until(done: impl Fn() -> bool) -> bool {
        let until = Instant::now() + Duration::from_secs(3);
        while !done() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
        done()
    }

    const MADE_AT: Area = Area { left: 100.0, top: 100.0, right: 740.0, bottom: 580.0 };

    /// One test with stream windows at a time: a concealer sees every stream window of this
    /// process, the other tests' too.
    fn one_at_a_time() -> std::sync::MutexGuard<'static, ()> {
        static STREAM_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());
        STREAM_TESTS.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn in_sight(r: Option<Area>) -> bool {
        r.is_some_and(|r| crate::layout::shows_on(r, &monitors()))
    }

    /// The viewer's stream window is out of sight the moment it is made: on the viewer's own
    /// thread, before `CreateWindowEx` returns there (the in-context hook,
    /// `expand_viewer_hook.dll`), so it is never on a monitor before it is embedded. Given
    /// back (the fallback), it comes back in sight.
    #[test]
    fn a_stream_window_is_out_of_sight_the_moment_it_is_made_until_it_is_given_back() {
        let _serial = one_at_a_time();
        assert!(viewer_hook_library().is_some(), "expand_viewer_hook.dll is not beside the tests (cargo test --workspace builds it)");
        let concealer = launcher_concealer();
        let (stream, made_at) = StreamWindow::make_full(w!("SDL_app"), MADE_AT, false, false);
        concealer.reveal();
        let back = wait_until(|| in_sight(window_rect(stream.window)));
        stream.end();
        assert!(made_at.is_some() && !in_sight(made_at), "the stream window was made in sight, at {made_at:?}");
        assert!(back, "the stream window did not come back in sight");
    }

    /// A run of the viewer before its stream (its paired check, its pairing) gets no in-context
    /// hook: its windows are watched from outside only.
    #[test]
    fn a_run_before_the_stream_gets_no_in_context_hook() {
        let _serial = one_at_a_time();
        let concealer = Concealer::start_with(std::process::id(), Talkers::OfClass(LAUNCHER), None, false);
        let (stream, made_at) = StreamWindow::make_full(w!("SDL_app"), MADE_AT, false, false);
        drop(concealer);
        stream.end();
        assert_eq!(made_at, Some(MADE_AT), "the window was moved inside its own making");
    }

    /// The viewer moving its window back into sight a moment after making it: out of sight again
    /// before its own move returns. Given back, it comes back in sight.
    #[test]
    fn a_stream_window_the_viewer_moves_back_is_out_of_sight_before_its_move_returns() {
        let _serial = one_at_a_time();
        let concealer = launcher_concealer();
        let (stream, made_at) = StreamWindow::make_full(w!("SDL_app"), MADE_AT, false, false);
        let moved = Area { left: 300.0, top: 200.0, right: 940.0, bottom: 680.0 };
        stream.moves.send(moved).unwrap();
        let seen = wait_until(|| stream.after_move.lock().unwrap().is_some());
        let after_move = *stream.after_move.lock().unwrap();
        concealer.reveal();
        let back = wait_until(|| in_sight(window_rect(stream.window)));
        stream.end();
        assert!(!in_sight(made_at), "made in sight");
        assert!(seen && !in_sight(after_move), "the viewer's move left it in sight, at {after_move:?}");
        assert!(back, "the stream window did not come back in sight");
    }

    /// Where a window goes back in sight when where it was is not known: the main monitor's middle.
    #[test]
    fn a_window_given_back_from_nowhere_known_goes_to_the_middle_of_the_main_monitor() {
        let monitors = [Area { left: -1920.0, top: 0.0, right: 0.0, bottom: 1080.0 }, Area { left: 0.0, top: 0.0, right: 2560.0, bottom: 1440.0 }];
        let w = Area { left: -14000.0, top: 431.0, right: -14000.0 + 1480.0, bottom: 431.0 + 860.0 };
        assert_eq!(in_sight_place(w, &monitors), Some(Area { left: 540.0, top: 290.0, right: 2020.0, bottom: 1150.0 }));
        assert_eq!(in_sight_place(w, &[]), None);
    }

    /// The class of the stand-in for the viewer's launcher (the window it speaks in).
    const LAUNCHER: &str = "ExpandTestLauncher";

    fn launcher_concealer() -> Concealer {
        Concealer::start_with(std::process::id(), Talkers::OfClass(LAUNCHER), None, true)
    }

    /// A concealer that tells what it hears on the returned channel.
    fn telling_concealer() -> (Concealer, std::sync::mpsc::Receiver<Heard>) {
        let (tx, rx) = std::sync::mpsc::channel();
        let tx = std::sync::Mutex::new(tx);
        let concealer = Concealer::start_with(
            std::process::id(),
            Talkers::OfClass(LAUNCHER),
            Some(Box::new(move |h| {
                let _ = tx.lock().unwrap().send(h);
            })),
            true,
        );
        (concealer, rx)
    }

    /// A window of the viewer that never shows (Qt's helpers, the launcher a hidden start keeps
    /// hidden) is left alone.
    #[test]
    fn a_window_of_the_viewer_that_never_shows_is_left_alone() {
        let _serial = one_at_a_time();
        let concealer = launcher_concealer();
        let other = StreamWindow::make_as(w!("ExpandTestLauncher"), MADE_AT, false);
        std::thread::sleep(Duration::from_millis(300));
        let at = window_rect(other.window);
        drop(concealer);
        other.end();
        assert_eq!(at, Some(MADE_AT));
    }

    /// The viewer showing a window it speaks in (its launcher with an error, a message box) is
    /// told of at once, and the window is left where the user can see it.
    #[test]
    fn a_window_the_viewer_speaks_in_is_told_of_and_left_in_sight() {
        let _serial = one_at_a_time();
        let (concealer, heard) = telling_concealer();
        let launcher = StreamWindow::make_as(w!("ExpandTestLauncher"), MADE_AT, true);
        let told = heard.recv_timeout(Duration::from_secs(3)).ok();
        std::thread::sleep(Duration::from_millis(200));
        let at = window_rect(launcher.window);
        drop(concealer);
        let window = launcher.window;
        launcher.end();
        assert_eq!(told, Some(Heard::Speaks(window)));
        assert_eq!(at, Some(MADE_AT), "the window the viewer speaks in was moved");
    }

    /// The fallback shows the launcher a hidden start kept hidden (the viewer thinks it shows,
    /// and never shows it again by itself; its errors are in it).
    #[test]
    fn the_fallback_shows_the_launcher_a_hidden_start_kept_hidden() {
        let _serial = one_at_a_time();
        let concealer = launcher_concealer();
        let launcher = StreamWindow::make_ghost(w!("ExpandTestLauncher"), MADE_AT, false, true);
        std::thread::sleep(Duration::from_millis(100));
        let hidden_before = !is_visible(launcher.window);
        concealer.reveal();
        let shown = wait_until(|| is_visible(launcher.window));
        launcher.end();
        assert!(hidden_before);
        assert!(shown, "the launcher stayed hidden");
    }

    /// The fallback leaves hidden a launcher the viewer showed and then hid itself (as it does
    /// when its stream starts).
    #[test]
    fn the_fallback_leaves_hidden_a_launcher_the_viewer_hid_itself() {
        use windows::Win32::UI::WindowsAndMessaging::ShowWindowAsync;
        let _serial = one_at_a_time();
        let (concealer, heard) = telling_concealer();
        let launcher = StreamWindow::make_as(w!("ExpandTestLauncher"), MADE_AT, true);
        let told = heard.recv_timeout(Duration::from_secs(3)).is_ok();
        unsafe {
            let _ = ShowWindowAsync(hwnd(launcher.window), SW_HIDE);
        }
        let hid = wait_until(|| !is_visible(launcher.window));
        concealer.reveal();
        std::thread::sleep(Duration::from_millis(300));
        let shown = is_visible(launcher.window);
        launcher.end();
        assert!(told && hid);
        assert!(!shown, "the fallback showed a launcher the viewer had hidden");
    }

    /// Once a stream window showed, the fallback leaves the launcher hidden: the viewer hides it
    /// for its stream, and the stream goes on in the viewer's own window.
    #[test]
    fn the_fallback_leaves_the_launcher_hidden_once_a_stream_window_showed() {
        let _serial = one_at_a_time();
        let (concealer, heard) = telling_concealer();
        let launcher = StreamWindow::make_ghost(w!("ExpandTestLauncher"), MADE_AT, false, true);
        let stream = StreamWindow::make_as(w!("SDL_app"), MADE_AT, true);
        let told = heard.recv_timeout(Duration::from_secs(3)).ok();
        concealer.reveal();
        std::thread::sleep(Duration::from_millis(300));
        let shown = is_visible(launcher.window);
        let window = stream.window;
        stream.end();
        launcher.end();
        assert_eq!(told, Some(Heard::StreamShown(window)));
        assert!(!shown, "the fallback showed the launcher beside the stream");
    }

    /// A message box's text is read from its static texts (on another thread, as Moonlight's
    /// would be), one per line, without its buttons.
    #[test]
    fn a_message_box_says_what_its_static_texts_say() {
        use windows::Win32::UI::WindowsAndMessaging::{WS_CHILD, WS_POPUP};
        let (tx, rx) = std::sync::mpsc::channel();
        let (stop, stopped) = std::sync::mpsc::channel::<()>();
        let thread = std::thread::spawn(move || unsafe {
            let dialog = CreateWindowExW(WINDOW_EX_STYLE(0), w!("#32770"), w!("Moonlight"), WS_POPUP, 0, 0, 300, 120, None, None, None, None)
                .expect("a dialog");
            for (text, class) in [(w!("Failed to start the stream"), w!("Static")), (w!("Error 42"), w!("Static")), (w!("OK"), w!("Button"))] {
                let _ = CreateWindowExW(WINDOW_EX_STYLE(0), class, text, WS_CHILD, 0, 0, 200, 20, dialog, None, None, None);
            }
            tx.send(dialog.0 as isize).unwrap();
            while stopped.try_recv().is_err() {
                pump();
                std::thread::sleep(Duration::from_millis(5));
            }
            let _ = DestroyWindow(dialog);
        });
        let dialog = rx.recv().unwrap();
        let text = message_text(dialog);
        let is_box = is_message_box(dialog);
        stop.send(()).unwrap();
        thread.join().unwrap();
        assert!(is_box);
        assert_eq!(text.as_deref(), Some("Failed to start the stream
Error 42"));
    }

    /// The stream window is told of the moment it shows, so it is embedded at once.
    #[test]
    fn a_stream_window_that_shows_is_told_of() {
        let _serial = one_at_a_time();
        let (concealer, heard) = telling_concealer();
        let stream = StreamWindow::make_as(w!("SDL_app"), MADE_AT, true);
        let told = heard.recv_timeout(Duration::from_secs(3)).ok();
        drop(concealer);
        let window = stream.window;
        stream.end();
        assert_eq!(told, Some(Heard::StreamShown(window)));
    }

    /// As a window, the display window's client area reaches its very top edge: no strip of frame
    /// above its title bar (seen live: a grey line at the top, 1 px at 125%). The sides and bottom
    /// keep what the window's own handling gives them.
    #[test]
    fn a_watched_window_has_no_frame_above_its_client_area() {
        use windows::Win32::UI::WindowsAndMessaging::{SWP_FRAMECHANGED, SWP_NOMOVE, SWP_NOSIZE};
        // A frame with a caption: Windows' own handling gives it a title bar's worth on top.
        let window = new_window();
        let before = (window_rect(window).unwrap(), client_area(window).unwrap());
        watch_frame(window);
        unsafe {
            let _ = SetWindowPos(hwnd(window), None, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED);
        }
        let (rect, client) = (window_rect(window).unwrap(), client_area(window).unwrap());
        unsafe {
            let _ = DestroyWindow(hwnd(window));
        }
        assert!(before.1.top > before.0.top, "the test window had no frame on top to begin with");
        assert_eq!(client.top, rect.top, "a frame is left above the client area");
        assert_eq!((client.left, client.right, client.bottom), (before.1.left, before.1.right, before.1.bottom));
    }

    /// The window goes into its parent straight at its place there: never a moment where it was.
    #[test]
    fn an_embedded_window_is_at_its_place_as_soon_as_it_is_in() {
        let _serial = one_at_a_time();
        let stream = StreamWindow::make(MADE_AT);
        let parent = new_window();
        let at = Area { left: 0.0, top: 40.0, right: 320.0, bottom: 220.0 };
        embed_at(stream.window, parent, at).expect("embedded");
        let client = client_area(parent).expect("a client area");
        let placed = window_rect(stream.window).map(|r| Area {
            left: r.left - client.left,
            top: r.top - client.top,
            right: r.right - client.left,
            bottom: r.bottom - client.top,
        });
        stream.end();
        unsafe {
            let _ = DestroyWindow(hwnd(parent));
        }
        assert_eq!(placed, Some(at));
    }
}
