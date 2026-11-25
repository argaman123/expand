//! Dev probe (Windows): can this process make another process's window invisible without waiting
//! for that process's thread? Tries DWM cloaking and a window region on a window of a child
//! process whose thread is kept busy (as Moonlight's is while its stream window appears).
//!
//! `cloak_probe` starts itself as `child`, which makes a hidden window far off every monitor and
//! then blocks its thread; the parent then asks, and reads back what took.

#[cfg(windows)]
fn main() {
    use std::time::{Duration, Instant};
    use windows::core::w;
    use windows::Win32::Foundation::{BOOL, HWND};
    use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DwmSetWindowAttribute, DWMWA_CLOAK, DWMWA_CLOAKED};
    use windows::Win32::Graphics::Gdi::{CreateRectRgn, GetWindowRgn, SetWindowRgn, GDI_REGION_TYPE};
    use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, FindWindowW, WINDOW_EX_STYLE, WS_OVERLAPPEDWINDOW};

    let args: Vec<String> = std::env::args().collect();
    if args.get(1).is_some_and(|a| a == "child") {
        unsafe {
            let _ = CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), w!("expand-cloak-probe"), WS_OVERLAPPEDWINDOW, -30000, -30000, 200, 100, None, None, None, None);
        }
        // Busy: never pumps messages.
        std::thread::sleep(Duration::from_secs(5));
        return;
    }
    let me = std::env::current_exe().unwrap();
    let mut child = std::process::Command::new(me).arg("child").spawn().unwrap();
    let mut window = HWND::default();
    for _ in 0..50 {
        std::thread::sleep(Duration::from_millis(50));
        if let Ok(w) = unsafe { FindWindowW(None, w!("expand-cloak-probe")) } {
            window = w;
            break;
        }
    }
    assert!(!window.0.is_null(), "no window");
    let on: BOOL = true.into();
    let t = Instant::now();
    let cloak = unsafe { DwmSetWindowAttribute(window, DWMWA_CLOAK, &on as *const _ as _, 4) };
    let took = t.elapsed();
    let mut cloaked = 0u32;
    let _ = unsafe { DwmGetWindowAttribute(window, DWMWA_CLOAKED, &mut cloaked as *mut _ as _, 4) };
    println!("cross-process DWMWA_CLOAK: {cloak:?} in {took:?}; DWMWA_CLOAKED reads {cloaked}");
    let t = Instant::now();
    let set = unsafe { SetWindowRgn(window, CreateRectRgn(0, 0, 0, 0), false) };
    let took = t.elapsed();
    let rgn = unsafe { CreateRectRgn(0, 0, 1, 1) };
    let kind: GDI_REGION_TYPE = unsafe { GetWindowRgn(window, rgn) };
    println!("cross-process SetWindowRgn: {set} in {took:?}; GetWindowRgn type {}", kind.0);
    let _ = child.kill();
}

#[cfg(not(windows))]
fn main() {}
