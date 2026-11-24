//! Dev probe (Windows): sets the in-context hook (`sidescreen_viewer_hook.dll`, beside this
//! program, as the app does) on every Moonlight.exe process that starts in the next `seconds`, as
//! the app's concealer does for its viewer, so its effect can be measured live against a Moonlight
//! started by an installed app that hasn't got it (with `window_sampler` alongside).
//!
//! `hook_probe <seconds>`

#[cfg(windows)]
fn main() {
    use std::collections::HashSet;
    use std::time::{Duration, Instant};
    use windows::core::{s, HSTRING};
    use windows::Win32::Foundation::{CloseHandle, HMODULE};
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, WINEVENTPROC};
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, PeekMessageW, EVENT_OBJECT_CREATE, EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_SHOW, MSG, PM_REMOVE,
        WINEVENT_INCONTEXT,
    };

    let secs: u64 = std::env::args().nth(1).unwrap().parse().unwrap();
    let path = std::env::current_exe().unwrap().with_file_name("sidescreen_viewer_hook.dll");
    let path = if path.is_file() { path } else { std::env::current_exe().unwrap().parent().unwrap().with_file_name("sidescreen_viewer_hook.dll") };
    let (module, proc): (HMODULE, WINEVENTPROC) = unsafe {
        let module = LoadLibraryW(&HSTRING::from(path.as_os_str())).expect("the hook library");
        let hook = GetProcAddress(module, s!("sidescreen_viewer_event")).expect("its hook");
        (module, std::mem::transmute::<usize, WINEVENTPROC>(hook as usize))
    };
    println!("hooking new Moonlight.exe processes for {secs} s with {}", path.display());
    let moonlights = || -> Vec<u32> {
        let mut out = Vec::new();
        unsafe {
            let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return out };
            let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
            let mut ok = Process32FirstW(snap, &mut e).is_ok();
            while ok {
                let n = e.szExeFile.iter().position(|c| *c == 0).unwrap_or(e.szExeFile.len());
                if String::from_utf16_lossy(&e.szExeFile[..n]).eq_ignore_ascii_case("Moonlight.exe") {
                    out.push(e.th32ProcessID);
                }
                ok = Process32NextW(snap, &mut e).is_ok();
            }
            let _ = CloseHandle(snap);
        }
        out
    };
    let started = Instant::now();
    let mut hooked: HashSet<u32> = HashSet::new();
    let mut hooks = Vec::new();
    while started.elapsed() < Duration::from_secs(secs) {
        for pid in moonlights() {
            if hooked.insert(pid) {
                for (from, to) in [(EVENT_OBJECT_CREATE, EVENT_OBJECT_SHOW), (EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_LOCATIONCHANGE)] {
                    let h = unsafe { SetWinEventHook(from, to, module, proc, pid, 0, WINEVENT_INCONTEXT) };
                    println!("{:>6} ms  hooked Moonlight pid {pid}: {}", started.elapsed().as_millis(), if h.is_invalid() { "FAILED" } else { "ok" });
                    hooks.push(h);
                }
            }
        }
        let mut msg = MSG::default();
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                DispatchMessageW(&msg);
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    for h in hooks {
        unsafe {
            let _ = UnhookWinEvent(h);
        }
    }
}

#[cfg(not(windows))]
fn main() {}
