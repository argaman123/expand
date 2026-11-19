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

pub fn is_visible(window: isize) -> bool {
    unsafe { IsWindowVisible(hwnd(window)).as_bool() }
}

/// If one of this app's windows is in front, puts the desktop in front instead.
pub fn step_aside_if_in_front() {
    use windows::Win32::UI::WindowsAndMessaging::GetShellWindow;
    unsafe {
        if process_of(GetForegroundWindow()) == std::process::id() {
            let _ = SetForegroundWindow(GetShellWindow());
        }
    }
}

pub fn bring_to_front(window: isize) {
    unsafe {
        let _ = SetForegroundWindow(hwnd(window));
    }
}

/// Asks the viewer to switch between full screen and a window, with its own shortcut
/// (Ctrl+Alt+Shift+X).
pub fn toggle_viewer_full_screen(viewer: isize) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VIRTUAL_KEY,
        VK_CONTROL, VK_MENU, VK_SHIFT,
    };
    bring_to_front(viewer);
    let key = |vk: VIRTUAL_KEY, flags: KEYBD_EVENT_FLAGS| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, wScan: 0, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
    };
    let x = VIRTUAL_KEY(b'X' as u16);
    let down = KEYBD_EVENT_FLAGS(0);
    let presses = [
        key(VK_CONTROL, down),
        key(VK_MENU, down),
        key(VK_SHIFT, down),
        key(x, down),
        key(x, KEYEVENTF_KEYUP),
        key(VK_SHIFT, KEYEVENTF_KEYUP),
        key(VK_MENU, KEYEVENTF_KEYUP),
        key(VK_CONTROL, KEYEVENTF_KEYUP),
    ];
    unsafe {
        SendInput(&presses, std::mem::size_of::<INPUT>() as i32);
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
