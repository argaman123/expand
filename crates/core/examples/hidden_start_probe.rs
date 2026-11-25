//! Dev probe (Windows): what a hidden start (`STARTUPINFO` with `SW_HIDE`, as `hidden::Child`
//! does) does to the windows a program shows: the first one stays hidden; does a later one?
//!
//! `hidden_start_probe` starts itself hidden as `child <first cmd> <second cmd>`, which makes two
//! windows far off every monitor, shows them with those `ShowWindow` commands, and gives back
//! which of them are visible in its exit code (bit 0 the first, bit 1 the second).

#[cfg(windows)]
fn main() {
    use windows::core::w;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, IsWindowVisible, ShowWindow, SHOW_WINDOW_CMD, SW_SHOW, SW_SHOWNA, SW_SHOWNORMAL, WINDOW_EX_STYLE,
        WS_OVERLAPPEDWINDOW,
    };
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).is_some_and(|a| a == "child") {
        let cmd = |i: usize| SHOW_WINDOW_CMD(args[i].parse::<i32>().unwrap());
        let make = || unsafe {
            CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), w!("probe"), WS_OVERLAPPEDWINDOW, -30000, -30000, 200, 100, None, None, None, None)
                .unwrap()
        };
        let (a, b) = (make(), make());
        let code = unsafe {
            let _ = ShowWindow(a, cmd(2));
            let _ = ShowWindow(b, cmd(3));
            IsWindowVisible(a).as_bool() as i32 | (IsWindowVisible(b).as_bool() as i32) << 1
        };
        std::process::exit(code);
    }
    let me = std::env::current_exe().unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    for (first, second) in [(SW_SHOWNORMAL, SW_SHOW), (SW_SHOWNORMAL, SW_SHOWNA), (SW_SHOW, SW_SHOW), (SW_SHOWNORMAL, SW_SHOWNORMAL)] {
        let args = vec!["child".to_string(), first.0.to_string(), second.0.to_string()];
        let child = expand_core::moonlight::hidden::Child::spawn(&me, &args).unwrap();
        let code = rt.block_on(child.wait()).unwrap();
        println!(
            "first ShowWindow({}) -> {}, then ShowWindow({}) -> {}",
            first.0,
            if code & 1 != 0 { "shown" } else { "hidden" },
            second.0,
            if code & 2 != 0 { "shown" } else { "hidden" }
        );
    }
}

#[cfg(not(windows))]
fn main() {}
