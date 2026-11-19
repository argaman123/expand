//! The top bar over the full-screen viewer (PC only): a slim handle at the top centre of the
//! viewer that opens into Settings, Windowed, Other monitor and Disconnect. It shows while a
//! session is connected, the viewer fills a monitor, and the viewer (or the bar) is in front.

use tauri::AppHandle;

pub const LABEL: &str = "bar";

/// Logical sizes: the closed handle, and the open bar.
#[cfg(windows)]
const CLOSED: (f64, f64) = (120.0, 7.0);
#[cfg(windows)]
const OPEN: (f64, f64) = (440.0, 40.0);

#[cfg(windows)]
static IS_OPEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(windows)]
pub fn create(app: &AppHandle) -> tauri::Result<()> {
    tauri::WebviewWindowBuilder::new(app, LABEL, tauri::WebviewUrl::App("bar.html".into()))
        .title("Sidescreen bar")
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .shadow(false)
        .focused(false)
        .visible(false)
        .inner_size(CLOSED.0, CLOSED.1)
        .build()?;
    // Windows gives a new window a minimum height; sizing it again afterwards is allowed.
    if let Some(bar) = tauri::Manager::get_webview_window(app, LABEL) {
        let _ = bar.set_size(tauri::LogicalSize::new(CLOSED.0, CLOSED.1));
    }
    Ok(())
}

/// Keeps the bar shown, hidden and placed as the viewer comes and goes.
#[cfg(windows)]
pub async fn keep_placed(app: AppHandle) {
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        place(&app);
    }
}

#[cfg(windows)]
fn viewer() -> Option<isize> {
    use sidescreen_core::winwindow;
    winwindow::find_window_of_process(winwindow::VIEWER_EXE)
}

#[cfg(windows)]
fn place(app: &AppHandle) {
    use sidescreen_core::layout::{bar_origin, filled_monitor};
    use sidescreen_core::session::State;
    use sidescreen_core::winwindow;
    use tauri::Manager;

    let Some(bar) = app.get_webview_window(LABEL) else { return };
    let shared = app.state::<crate::Shared>();
    let connected = matches!(*shared.state.borrow(), State::Connected);
    let bar_window = bar.hwnd().ok().map(|h| h.0 as isize);
    let front = winwindow::foreground_window();
    let open = IS_OPEN.load(std::sync::atomic::Ordering::Relaxed);
    // Nothing to look for while disconnected.
    let full_screen_viewer = connected
        .then(viewer)
        .flatten()
        .and_then(|v| winwindow::window_rect(v).map(|r| (v, r)))
        .filter(|(_, r)| filled_monitor(*r, &winwindow::monitors()).is_some())
        // In front, or being used: the bar only counts while it is open.
        .filter(|(v, _)| front == *v || (open && Some(front) == bar_window));
    match (full_screen_viewer, bar_window) {
        (Some((_, rect)), Some(window)) => {
            if let Ok(size) = bar.outer_size() {
                let (x, y) = bar_origin(rect, (size.width as f64, size.height as f64));
                let _ = bar.set_position(tauri::PhysicalPosition::new(x as i32, y as i32));
            }
            // Shown and hidden through Windows directly (so it never takes the keyboard from the
            // viewer); Tauri's own idea of whether it is visible does not see that.
            if !winwindow::is_visible(window) {
                winwindow::show_without_focus(window);
            }
        }
        (_, window) => {
            if let Some(window) = window.filter(|w| winwindow::is_visible(*w)) {
                winwindow::hide(window);
            }
            // Next time it shows, it shows as the thin handle.
            if IS_OPEN.swap(false, std::sync::atomic::Ordering::Relaxed) {
                let _ = bar.set_size(tauri::LogicalSize::new(CLOSED.0, CLOSED.1));
                let _ = tauri::Emitter::emit_to(&bar, LABEL, "bar-closed", ());
            }
        }
    }
}

/// The bar opens while the pointer is over it, and closes after it leaves.
#[tauri::command]
pub fn bar_expand(app: AppHandle, expanded: bool) {
    #[cfg(windows)]
    {
        use sidescreen_core::winwindow;
        use tauri::Manager;
        IS_OPEN.store(expanded, std::sync::atomic::Ordering::Relaxed);
        let Some(bar) = app.get_webview_window(LABEL) else { return };
        let (w, h) = if expanded { OPEN } else { CLOSED };
        let _ = bar.set_size(tauri::LogicalSize::new(w, h));
        place(&app);
        // Closing hands the keyboard back to the viewer if the bar had taken it.
        let bar_window = bar.hwnd().ok().map(|h| h.0 as isize);
        if !expanded && Some(winwindow::foreground_window()) == bar_window {
            if let Some(v) = viewer() {
                winwindow::bring_to_front(v);
            }
        }
    }
    #[cfg(not(windows))]
    let _ = (app, expanded);
}

#[tauri::command]
pub fn bar_settings(app: AppHandle) {
    use tauri::Manager;
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.unminimize();
        let _ = main.show();
        let _ = main.set_focus();
    }
}

/// Switches the viewer between full screen and a window.
#[tauri::command]
pub fn bar_windowed() {
    #[cfg(windows)]
    if let Some(v) = viewer() {
        sidescreen_core::winwindow::toggle_viewer_full_screen(v);
    }
}

/// The full-screen viewer and the monitor it would move to, if there is another one.
#[cfg(windows)]
fn move_target() -> Option<(isize, sidescreen_core::share::geometry::Area)> {
    use sidescreen_core::layout::{filled_monitor, next_monitor};
    use sidescreen_core::winwindow;
    let v = viewer()?;
    let monitors = winwindow::monitors();
    let here = winwindow::window_rect(v).and_then(|r| filled_monitor(r, &monitors))?;
    next_monitor(&monitors, here).map(|next| (v, next))
}

/// Whether "Other monitor" makes sense: only with more than one monitor.
#[tauri::command]
pub fn bar_can_move() -> bool {
    #[cfg(windows)]
    return move_target().is_some();
    #[cfg(not(windows))]
    false
}

/// Moves the full-screen viewer to the next monitor.
#[tauri::command]
pub fn bar_move() {
    #[cfg(windows)]
    if let Some((v, next)) = move_target() {
        sidescreen_core::winwindow::move_window(v, next);
    }
}
