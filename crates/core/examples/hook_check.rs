//! Dev probe (Windows): can this hook library be loaded and set in context? Loads it, finds its
//! hook, and sets it on this process only (nothing goes into another process).
//!
//! `hook_check <path to sidescreen_viewer_hook.dll>`

#[cfg(windows)]
fn main() {
    use windows::core::{s, HSTRING};
    use windows::Win32::Foundation::GetLastError;
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, WINEVENTPROC};
    use windows::Win32::UI::WindowsAndMessaging::{EVENT_OBJECT_CREATE, WINEVENT_INCONTEXT};
    let path = std::env::args().nth(1).expect("a path");
    unsafe {
        let module = match LoadLibraryW(&HSTRING::from(path.as_str())) {
            Ok(m) => m,
            Err(e) => return println!("LoadLibraryW failed: {e}"),
        };
        println!("loaded at {:?}", module.0);
        let Some(hook) = GetProcAddress(module, s!("sidescreen_viewer_event")) else {
            return println!("no sidescreen_viewer_event: {:?}", GetLastError());
        };
        let proc: WINEVENTPROC = std::mem::transmute::<usize, WINEVENTPROC>(hook as usize);
        let h = SetWinEventHook(EVENT_OBJECT_CREATE, EVENT_OBJECT_CREATE, module, proc, std::process::id(), 0, WINEVENT_INCONTEXT);
        println!("SetWinEventHook in context: {}", if h.is_invalid() { format!("failed {:?}", GetLastError()) } else { "ok".into() });
        let _ = UnhookWinEvent(h);
    }
}

#[cfg(not(windows))]
fn main() {}
