//! Dev probe (Windows): starts Moonlight the way the app does and measures, by sampling every
//! top-level window of its process every 2 ms, how long any of them was shown on a monitor.
//!
//! `conceal_probe <Moonlight.exe> <list|pair|stream> <host> <seconds> [bare]`
//!
//! `bare` starts it as before round 3 (no hidden start, no concealer) for comparison. Run it from
//! a portable copy of Moonlight (a `portable.dat` beside it, and that folder as the working
//! directory) so the installed Moonlight's settings are never touched. With
//! `SIDESCREEN_TRACE_WINDOWS=1` and `SIDESCREEN_CONFIG` set, the concealer's own trace goes to
//! `windows-trace.log` beside that config.

#[cfg(windows)]
fn main() {
    use sidescreen_core::layout::shows_on;
    use sidescreen_core::share::geometry::Area;
    use std::collections::HashMap;
    use std::time::{Duration, Instant};
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
    use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetClassNameW, GetWindowRect, GetWindowThreadProcessId, IsWindowVisible};

    let args: Vec<String> = std::env::args().collect();
    let (exe, mode, host, secs) = (&args[1], args[2].as_str(), &args[3], args[4].parse::<u64>().unwrap());
    let bare = args.get(5).is_some_and(|a| a == "bare");
    // `reveal`: end as the display's fallback does (the windows given back), and see what shows.
    let reveal = args.get(5).is_some_and(|a| a == "reveal");
    sidescreen_core::sharelog::enable();

    let exe = std::path::Path::new(exe);
    let moonlight_args: Vec<String> = match mode {
        "list" => sidescreen_core::moonlight::list_args(host),
        // A PIN no Sunshine waits for.
        "pair" => sidescreen_core::moonlight::pair_args(host, "0000"),
        _ => vec!["stream".into(), host.clone(), "Desktop".into(), "--display-mode".into(), "windowed".into()],
    };
    enum Run {
        Hidden(sidescreen_core::moonlight::hidden::Child),
        Plain(std::process::Child),
    }
    let run = if (mode == "pair" || mode == "stream") && !bare {
        Run::Hidden(sidescreen_core::moonlight::hidden::Child::spawn(exe, &moonlight_args).unwrap())
    } else {
        Run::Plain(std::process::Command::new(exe).args(&moonlight_args).spawn().unwrap())
    };
    let pid = match &run {
        Run::Hidden(c) => c.pid(),
        Run::Plain(c) => c.id(),
    };
    let concealer = (!bare).then(|| sidescreen_core::winwindow::Concealer::start(pid));

    unsafe extern "system" fn collect(window: HWND, out: LPARAM) -> BOOL {
        let list = &mut *(out.0 as *mut Vec<isize>);
        list.push(window.0 as isize);
        BOOL(1)
    }
    let monitors = sidescreen_core::winwindow::monitors();
    let started = Instant::now();
    // Per window: its class, when it was last seen showing, and how long it showed in all.
    let mut seen: HashMap<isize, (String, Option<Instant>, Duration, u32)> = HashMap::new();
    while started.elapsed() < Duration::from_secs(secs) {
        let mut all: Vec<isize> = Vec::new();
        unsafe {
            let _ = EnumWindows(Some(collect), LPARAM(&mut all as *mut _ as isize));
        }
        let now = Instant::now();
        for w in all {
            let h = HWND(w as *mut _);
            let mut p = 0u32;
            unsafe { GetWindowThreadProcessId(h, Some(&mut p)) };
            if p != pid {
                continue;
            }
            let mut r = RECT::default();
            let _ = unsafe { GetWindowRect(h, &mut r) };
            let area = Area { left: r.left as f64, top: r.top as f64, right: r.right as f64, bottom: r.bottom as f64 };
            let showing = unsafe { IsWindowVisible(h).as_bool() } && shows_on(area, &monitors);
            let entry = seen.entry(w).or_insert_with(|| {
                let mut buf = [0u16; 128];
                let n = unsafe { GetClassNameW(h, &mut buf) }.max(0) as usize;
                (String::from_utf16_lossy(&buf[..n]), None, Duration::ZERO, 0)
            });
            match (showing, entry.1) {
                (true, None) => {
                    entry.1 = Some(now);
                    entry.3 += 1;
                    println!("{:>6} ms  {w:#x} {} shows at {area:?}", started.elapsed().as_millis(), entry.0);
                }
                (false, Some(since)) => {
                    entry.2 += now - since;
                    entry.1 = None;
                    println!("{:>6} ms  {w:#x} {} gone after {} ms", started.elapsed().as_millis(), entry.0, (now - since).as_millis());
                }
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    if reveal {
        if let Some(c) = concealer {
            c.reveal();
        }
        std::thread::sleep(Duration::from_millis(1500));
        let mut all: Vec<isize> = Vec::new();
        unsafe {
            let _ = EnumWindows(Some(collect), LPARAM(&mut all as *mut _ as isize));
        }
        for w in all {
            let h = HWND(w as *mut _);
            let mut p = 0u32;
            unsafe { GetWindowThreadProcessId(h, Some(&mut p)) };
            if p == pid && unsafe { IsWindowVisible(h).as_bool() } {
                let mut r = RECT::default();
                let _ = unsafe { GetWindowRect(h, &mut r) };
                println!("after the fallback: {w:#x} shows at {},{}", r.left, r.top);
            }
        }
    } else {
        drop(concealer);
    }
    match run {
        Run::Hidden(c) => drop(c),
        Run::Plain(mut c) => {
            let _ = c.kill();
        }
    }
    println!("--- {mode} {}: windows of pid {pid}", if bare { "bare" } else { "hidden and concealed" });
    for (w, (class, since, total, times)) in seen {
        let total = total + since.map_or(Duration::ZERO, |s| s.elapsed());
        println!("{w:#x} {class}: shown {times} time(s), {} ms in all", total.as_millis());
    }
}

#[cfg(not(windows))]
fn main() {}
