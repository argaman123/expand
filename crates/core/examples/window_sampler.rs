//! Dev probe (Windows): for `seconds`, every 5 ms, looks at every top-level window of every
//! process of `exe` (Moonlight.exe by default) and prints each stretch one is visible, not
//! cloaked and on a monitor (what the user can see), with how long it lasted.
//!
//! `window_sampler <seconds> [exe name]`

#[cfg(windows)]
fn main() {
    use expand_core::layout::shows_on;
    use expand_core::share::geometry::Area;
    use std::collections::HashMap;
    use std::time::{Duration, Instant};
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
    use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
    use windows::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
    use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetClassNameW, GetWindowRect, GetWindowThreadProcessId, IsWindowVisible};

    expand_core::winwindow::use_physical_pixels();
    let args: Vec<String> = std::env::args().collect();
    let secs: u64 = args[1].parse().unwrap();
    let exe = args.get(2).cloned().unwrap_or_else(|| "Moonlight.exe".into());
    unsafe extern "system" fn collect(window: HWND, out: LPARAM) -> BOOL {
        let list = &mut *(out.0 as *mut Vec<isize>);
        list.push(window.0 as isize);
        BOOL(1)
    }
    let exe_of = |pid: u32| -> String {
        unsafe {
            let Ok(p) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return String::new() };
            let mut buf = [0u16; 512];
            let mut len = buf.len() as u32;
            let _ = QueryFullProcessImageNameW(p, PROCESS_NAME_WIN32, windows::core::PWSTR(buf.as_mut_ptr()), &mut len);
            let _ = windows::Win32::Foundation::CloseHandle(p);
            String::from_utf16_lossy(&buf[..len as usize]).rsplit('\\').next().unwrap_or("").to_string()
        }
    };
    let monitors = expand_core::winwindow::monitors();
    let started = Instant::now();
    let mut names: HashMap<u32, bool> = HashMap::new();
    let mut since: HashMap<isize, (Instant, String, Area)> = HashMap::new();
    let mut seen_any: HashMap<isize, String> = HashMap::new();
    let mut shown = 0;
    // The stream window's place and scale as Windows sees them, while it is top-level.
    let mut stream_seen: HashMap<isize, (Area, u32)> = HashMap::new();
    let dpi_of = |w: isize| unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(HWND(w as *mut _)) };
    println!("sampling {exe} windows for {secs} s");
    while started.elapsed() < Duration::from_secs(secs) {
        let mut all: Vec<isize> = Vec::new();
        unsafe {
            let _ = EnumWindows(Some(collect), LPARAM(&mut all as *mut _ as isize));
        }
        let now = Instant::now();
        let mut showing_now: Vec<isize> = Vec::new();
        for w in all {
            let h = HWND(w as *mut _);
            let mut pid = 0u32;
            unsafe { GetWindowThreadProcessId(h, Some(&mut pid)) };
            if !*names.entry(pid).or_insert_with(|| exe_of(pid).eq_ignore_ascii_case(&exe)) {
                continue;
            }
            let mut class = [0u16; 128];
            let n = unsafe { GetClassNameW(h, &mut class) }.max(0) as usize;
            let class = String::from_utf16_lossy(&class[..n]);
            if !seen_any.contains_key(&w) {
                println!("{:>6} ms  pid {pid} window {w:#x} {class} exists", started.elapsed().as_millis());
                seen_any.insert(w, class.clone());
            }
            let mut r = RECT::default();
            let _ = unsafe { GetWindowRect(h, &mut r) };
            let area = Area { left: r.left as f64, top: r.top as f64, right: r.right as f64, bottom: r.bottom as f64 };
            if class == "SDL_app" {
                let now_at = (area, dpi_of(w));
                if stream_seen.get(&w) != Some(&now_at) {
                    println!("{:>6} ms  {w:#x} SDL_app at {:?} {}x{}, scale {} %", started.elapsed().as_millis(), (area.left, area.top), area.right - area.left, area.bottom - area.top, now_at.1 * 100 / 96);
                    stream_seen.insert(w, now_at);
                }
            }
            let mut cloaked = 0u32;
            let _ = unsafe { DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut cloaked as *mut _ as _, 4) };
            if unsafe { IsWindowVisible(h).as_bool() } && cloaked == 0 && shows_on(area, &monitors) {
                showing_now.push(w);
                since.entry(w).or_insert_with(|| {
                    println!("{:>6} ms  SHOWS {w:#x} {class} at {area:?}", started.elapsed().as_millis());
                    (now, class.clone(), area)
                });
            }
        }
        // Windows no longer top-level: embedded in another window (still a window) or destroyed.
        let listed: Vec<isize> = {
            let mut all: Vec<isize> = Vec::new();
            unsafe {
                let _ = EnumWindows(Some(collect), LPARAM(&mut all as *mut _ as isize));
            }
            all
        };
        let left: Vec<isize> = seen_any.keys().copied().filter(|w| !listed.contains(w)).collect();
        for w in left {
            let class = seen_any.remove(&w).unwrap();
            let window = unsafe { windows::Win32::UI::WindowsAndMessaging::IsWindow(HWND(w as *mut _)).as_bool() };
            if class == "SDL_app" || window {
                println!("{:>6} ms  {w:#x} {class} is no longer top-level: {}", started.elapsed().as_millis(), if window { "embedded in another window" } else { "destroyed" });
                if window {
                    // A moment for the display window to place it.
                    std::thread::sleep(Duration::from_millis(3000));
                    let h = HWND(w as *mut _);
                    let parent = unsafe { windows::Win32::UI::WindowsAndMessaging::GetParent(h) }.unwrap_or_default();
                    let mut c = RECT::default();
                    let mut p = RECT::default();
                    unsafe {
                        let _ = windows::Win32::UI::WindowsAndMessaging::GetClientRect(h, &mut c);
                        let _ = windows::Win32::UI::WindowsAndMessaging::GetClientRect(parent, &mut p);
                    }
                    println!(
                        "          embedded: stream {}x{} at scale {} %, in a display window {}x{} at scale {} %",
                        c.right, c.bottom, dpi_of(w) * 100 / 96, p.right, p.bottom, dpi_of(parent.0 as isize) * 100 / 96
                    );
                }
            }
        }
        let gone: Vec<isize> = since.keys().copied().filter(|w| !showing_now.contains(w)).collect();
        for w in gone {
            let (from, class, _) = since.remove(&w).unwrap();
            shown += 1;
            println!("{:>6} ms  gone {w:#x} {class} after {} ms", started.elapsed().as_millis(), (now - from).as_millis());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    for (w, (from, class, area)) in since {
        shown += 1;
        println!("still showing at the end: {w:#x} {class} at {area:?} for {} ms", from.elapsed().as_millis());
    }
    println!("--- {shown} stretch(es) of a {exe} window on a monitor");
}

#[cfg(not(windows))]
fn main() {}
