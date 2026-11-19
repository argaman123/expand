//! The Sidescreen app: a small window and tray icon over `sidescreen_core`.
//! Windows builds are the client (the PC that shows the display); macOS builds are the host.

mod bar;
pub mod state;
mod system;
mod macmenu;

use serde::{Deserialize, Serialize};
use sidescreen_core::config::{Config, Peer, StreamSettings, WindowMode};
use sidescreen_core::side::Side;
use sidescreen_core::mouse::MouseManager;
use sidescreen_core::session::State;
use state::{AppState, StatusDto, ROLE};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, RunEvent, WindowEvent};
use tokio_util::sync::CancellationToken;

type Shared = Arc<AppState>;
/// The mouse-sharing engine. Async-locked because starting and stopping it takes time.
type Mouse = Arc<tokio::sync::Mutex<MouseManager>>;

/// Long-running work that must be wound down before the app exits.
struct Background {
    shutdown: CancellationToken,
    task: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
}

const EXIT_WAIT: Duration = Duration::from_secs(4);

#[derive(Debug, Deserialize)]
struct SettingsInput {
    width: u32,
    height: u32,
    fps: u32,
    bitrate_mbps: u32,
    window_mode: String,
}

#[derive(Debug, Serialize)]
struct FoundDto {
    name: String,
    host: String,
}

#[tauri::command]
fn get_status(shared: tauri::State<'_, Shared>) -> StatusDto {
    shared.status()
}

#[tauri::command]
fn connect(app: AppHandle, shared: tauri::State<'_, Shared>) -> Result<(), String> {
    client::start_session(&app, shared.inner().clone())
}

#[tauri::command]
fn disconnect(shared: tauri::State<'_, Shared>) {
    shared.cancel_session();
}

#[tauri::command]
async fn find_mac() -> Option<FoundDto> {
    sidescreen_core::discovery::find_first(Duration::from_secs(3))
        .await
        .map(|f| FoundDto { name: f.name, host: f.host })
}

#[tauri::command]
async fn set_peer_host(app: AppHandle, shared: tauri::State<'_, Shared>, host: String) -> Result<(), String> {
    if !shared.is_idle() {
        return Err("disconnect first".to_string());
    }
    shared.set_peer_host(&host)?;
    // A new address ends the old pairing, and sharing with it.
    sync_mouse(&app).await;
    Ok(())
}

#[tauri::command]
async fn save_settings(app: AppHandle, shared: tauri::State<'_, Shared>, settings: SettingsInput) -> Result<(), String> {
    let window_mode = match settings.window_mode.as_str() {
        "windowed" => WindowMode::Windowed,
        "borderless" => WindowMode::Borderless,
        other => return Err(format!("unknown window mode: {other}")),
    };
    shared.save_settings(StreamSettings {
        width: settings.width,
        height: settings.height,
        fps: settings.fps,
        bitrate_kbps: settings.bitrate_mbps.saturating_mul(1000),
        window_mode,
    })?;
    // Sharing places the pointer in the viewer by the stream's shape.
    sync_mouse(&app).await;
    Ok(())
}

#[tauri::command]
async fn forget_peer(app: AppHandle, shared: tauri::State<'_, Shared>) -> Result<(), String> {
    shared.forget_peer()?;
    // On the Mac, also drop the stream host's own pairing so the next PC pairs from scratch.
    if ROLE == "host" {
        let _ = std::fs::remove_file(sunshine_dir(&shared).join("sunshine_state.json"));
    }
    sync_mouse(&app).await;
    Ok(())
}

#[derive(Serialize)]
struct MonitorDto {
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
}

/// This PC's monitors, for the arrangement editor.
#[tauri::command]
fn get_monitors() -> Vec<MonitorDto> {
    #[cfg(windows)]
    return sidescreen_core::winwindow::monitors()
        .into_iter()
        .map(|m| MonitorDto { left: m.left, top: m.top, right: m.right, bottom: m.bottom })
        .collect();
    #[cfg(not(windows))]
    Vec::new()
}

/// Hides the window (the Mac's setup guide's Done button).
#[tauri::command]
fn close_window(window: tauri::WebviewWindow) {
    let _ = window.hide();
}

#[tauri::command]
async fn set_clipboard_sharing(app: AppHandle, shared: tauri::State<'_, Shared>, enabled: bool) -> Result<(), String> {
    shared.set_clipboard_sharing(enabled)?;
    sync_mouse(&app).await;
    Ok(())
}

#[tauri::command]
async fn set_language_follow(app: AppHandle, shared: tauri::State<'_, Shared>, enabled: bool) -> Result<(), String> {
    shared.set_language_follow(enabled)?;
    sync_mouse(&app).await;
    Ok(())
}

#[tauri::command]
fn set_close_to_tray(app: AppHandle, shared: tauri::State<'_, Shared>, enabled: bool) -> Result<(), String> {
    shared.set_close_to_tray(enabled)?;
    announce(&app, &shared);
    Ok(())
}

#[tauri::command]
async fn set_sharing_paused(app: AppHandle, shared: tauri::State<'_, Shared>, paused: bool) -> Result<(), String> {
    shared.set_sharing_paused(paused)?;
    sync_mouse(&app).await;
    Ok(())
}

#[tauri::command]
async fn set_mouse_sharing(app: AppHandle, shared: tauri::State<'_, Shared>, enabled: bool) -> Result<(), String> {
    shared.set_mouse_sharing(enabled)?;
    sync_mouse(&app).await;
    Ok(())
}

/// `side`: where the Mac sits relative to the PC.
#[tauri::command]
async fn set_mac_side(app: AppHandle, shared: tauri::State<'_, Shared>, side: String) -> Result<(), String> {
    let side = match side.as_str() {
        "left" => Side::Left,
        "right" => Side::Right,
        "top" => Side::Top,
        "bottom" => Side::Bottom,
        other => return Err(format!("unknown side: {other}")),
    };
    shared.set_mac_side(side)?;
    sync_mouse(&app).await;
    Ok(())
}

#[tauri::command]
async fn reset_config(app: AppHandle, shared: tauri::State<'_, Shared>) -> Result<(), String> {
    shared.reset_config()?;
    // The pairing is gone: the Mac must stop accepting the old PC at once.
    sync_mouse(&app).await;
    Ok(())
}

/// Opens the macOS privacy pane where a permission is granted. `kind`: "screen" or "accessibility".
#[tauri::command]
fn open_privacy_settings(kind: String) -> Result<(), String> {
    let pane = match kind.as_str() {
        "screen" => "Privacy_ScreenCapture",
        "accessibility" => "Privacy_Accessibility",
        other => return Err(format!("unknown permission: {other}")),
    };
    if cfg!(target_os = "macos") {
        std::process::Command::new("open")
            .arg(format!("x-apple.systempreferences:com.apple.preference.security?{pane}"))
            .status()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Brings the mouse-sharing engine in line with the current settings: running with the peer's
/// details, or off. Failures only change the mouse status, never the display session.
async fn sync_mouse(app: &AppHandle) {
    let shared = app.state::<Shared>().inner().clone();
    let mouse = app.state::<Mouse>().inner().clone();
    let mut manager = mouse.lock().await;
    match shared.mouse_role() {
        Some(role) => manager.start(role).await,
        None => manager.stop().await,
    }
    shared.set_mouse_status(manager.status());
    drop(manager);
    announce(app, &shared);
}


fn sunshine_dir(shared: &AppState) -> std::path::PathBuf {
    shared.path.parent().unwrap_or(std::path::Path::new(".")).join("sunshine")
}

fn announce(app: &AppHandle, shared: &AppState) {
    let status = shared.status();
    refresh_tray_menu(app, &status);
    if let Some(tray) = app.tray_by_id("main") {
        let text = if status.sharing_paused {
            state::tray_text_paused(status.state)
        } else {
            state::tray_text(status.state, status.mouse_state)
        };
        let _ = tray.set_tooltip(Some(text));
    }
    let _ = app.emit("state-changed", status);
}

fn show_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

mod client {
    use super::*;
    use sidescreen_core::link::CONTROL_PORT;
    use sidescreen_core::session::client::run;
    use sidescreen_core::session::engines::RealViewer;
    use std::net::{IpAddr, SocketAddr};

    /// Starts a session unless one is already running.
    pub fn start_session(app: &AppHandle, shared: Shared) -> Result<(), String> {
        if ROLE != "client" {
            return Err("connect from the PC".to_string());
        }
        let peer = shared.cfg.lock().unwrap().peer.clone().ok_or("choose a Mac first")?;
        // Everything the Mac can be reached at; whichever answers first is used.
        let ips: Vec<IpAddr> = peer.candidates();
        if ips.is_empty() {
            return Err("the saved Mac address is not valid".to_string());
        }
        let Some(cancel) = shared.begin_session() else {
            return Ok(());
        };
        let app = app.clone();
        // Not tracked by a handle: exit waits for `AppState`'s session slot to clear instead.
        tauri::async_runtime::spawn(async move {
            let cfg = shared.cfg.lock().unwrap().clone();
            match RealViewer::new(cfg.mac_side) {
                Ok(mut viewer) => {
                    let addrs: Vec<SocketAddr> = ips.iter().map(|ip| SocketAddr::new(*ip, CONTROL_PORT)).collect();
                    let pairing_app = app.clone();
                    let save_now = move |peer: Peer| {
                        let shared = pairing_app.state::<Shared>().inner().clone();
                        let _ = shared.set_peer(peer);
                        // A new pairing is what mouse sharing authenticates with.
                        let app = pairing_app.clone();
                        tauri::async_runtime::spawn(async move { sync_mouse(&app).await });
                    };
                    if let Some(peer) = run(addrs, &cfg, &mut viewer, &shared.state, cancel, &save_now).await {
                        let _ = shared.set_peer(peer);
                    }
                }
                Err(e) => {
                    shared.state.send_replace(State::Failed { part: "Moonlight".to_string(), reason: e.to_string() });
                }
            }
            shared.end_session();
            announce(&app, &shared);
        });
        Ok(())
    }
}

#[cfg(target_os = "macos")]
mod host {
    use super::*;
    use async_trait::async_trait;
    use sidescreen_core::discovery;
    use sidescreen_core::link::{server::Server, TrustPrompt, CONTROL_PORT};
    use sidescreen_core::session::engines::{cleanup_orphans, RealHostEngines};
    use sidescreen_core::session::host::serve;
    use sidescreen_core::session::HostEngines;
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};

    struct DialogPrompt {
        app: AppHandle,
    }

    #[async_trait]
    impl TrustPrompt for DialogPrompt {
        async fn ask(&self, name: &str, _id: &str) -> bool {
            // A menu-bar app is never in front by itself; bring it forward so the question shows.
            #[cfg(target_os = "macos")]
            let _ = self.app.show();
            let (tx, rx) = tokio::sync::oneshot::channel();
            self.app
                .dialog()
                .message(format!("Allow \"{name}\" to use this Mac's display and control it?"))
                .title("Sidescreen")
                .buttons(MessageDialogButtons::OkCancelCustom("Allow".to_string(), "Don't Allow".to_string()))
                .show(move |allowed| {
                    let _ = tx.send(allowed);
                });
            rx.await.unwrap_or(false)
        }
    }

    /// Listens for the PC and serves one session at a time until the app shuts down.
    pub async fn run(app: AppHandle, shared: Shared, shutdown: CancellationToken) {
        let server = match Server::bind(([0, 0, 0, 0], CONTROL_PORT).into()).await {
            Ok(server) => server,
            Err(e) => {
                let reason = format!("cannot listen for the PC (is Sidescreen already open?): {e}");
                shared.state.send_replace(State::Failed { part: "Mac app".to_string(), reason });
                return;
            }
        };
        let (name, id) = {
            let cfg = shared.cfg.lock().unwrap();
            (cfg.device_name.clone(), cfg.device_id.clone())
        };
        let _advertisement = discovery::advertise(&name, &id, CONTROL_PORT).ok();
        let side_shared = shared.clone();
        // The PC says which side of it this Mac sits on: where the virtual display goes.
        let on_arrangement = move |mac_side: Side| {
            if side_shared.cfg.lock().unwrap().mac_side != mac_side {
                let _ = side_shared.set_mac_side(mac_side);
            }
        };
        let prompt = DialogPrompt { app: app.clone() };

        // An earlier run that crashed or was force-quit may have left its engines behind.
        cleanup_orphans(&sunshine_dir(&shared)).await;

        loop {
            // The trusted peer is read when a PC actually connects, so Forget applies at once.
            let trusted = || shared.cfg.lock().unwrap().peer.clone();
            let accepted = tokio::select! {
                _ = shutdown.cancelled() => return,
                r = server.accept(trusted, &prompt) => r,
            };
            let Ok((conn, new_peer)) = accepted else {
                continue;
            };
            if let Some(peer) = new_peer {
                // A newly trusted PC must pair with the stream host from scratch.
                let _ = std::fs::remove_file(sunshine_dir(&shared).join("sunshine_state.json"));
                let _ = shared.set_peer(peer);
                // Mouse sharing now answers this PC.
                let app = app.clone();
                tauri::async_runtime::spawn(async move { sync_mouse(&app).await });
            }
            let (user, pass) = {
                let cfg = shared.cfg.lock().unwrap();
                (cfg.sunshine_user.clone(), cfg.sunshine_pass.clone())
            };
            let mut engines = RealHostEngines::new(sunshine_dir(&shared), user, pass);
            let interrupted = tokio::select! {
                _ = serve(conn, &mut engines, &shared.state, &on_arrangement) => false,
                _ = shutdown.cancelled() => true,
            };
            if interrupted {
                engines.stop().await;
                shared.state.send_replace(State::Disconnected);
                return;
            }
        }
    }
}

fn build_tray(app: &AppHandle, shared: &Shared) -> tauri::Result<()> {
    let menu = if ROLE == "client" {
        client_menu(app, &shared.status())?
    } else {
        // The Mac lives in the menu bar; its menu is filled in by `refresh_host_menu`.
        host_menu_items(app, &shared.status())?
    };

    let for_menu = shared.clone();
    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("Sidescreen")
        .menu(&menu)
        .show_menu_on_left_click(ROLE == "host")
        .on_menu_event(move |app, event| match event.id.as_ref() {
            "open" => show_window(app),
            "forget" => {
                let (app, shared) = (app.clone(), for_menu.clone());
                tauri::async_runtime::spawn(async move {
                    if shared.forget_peer().is_ok() {
                        let _ = std::fs::remove_file(sunshine_dir(&shared).join("sunshine_state.json"));
                    }
                    sync_mouse(&app).await;
                });
            }
            "pause" => {
                let (app, shared) = (app.clone(), for_menu.clone());
                let paused = !shared.cfg.lock().unwrap().sharing_paused;
                tauri::async_runtime::spawn(async move {
                    let _ = shared.set_sharing_paused(paused);
                    sync_mouse(&app).await;
                });
            }
            "login" => {
                let on = system::snapshot(app).start_at_login;
                let _ = system::set_start_at_login(app.clone(), !on);
                announce(app, &for_menu);
            }
            "perm-screen" => {
                let _ = open_privacy_settings("screen".to_string());
            }
            "perm-access" => {
                let _ = open_privacy_settings("accessibility".to_string());
            }
            "install" => {
                let _ = system::install_missing();
            }
            "connect" => {
                let _ = client::start_session(app, for_menu.clone());
            }
            "disconnect" => {
                for_menu.cancel_session();
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            // On the PC a click opens the window; on the Mac it opens the menu.
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                if ROLE == "client" {
                    show_window(tray.app_handle());
                }
            }
        });
    // The Mac's menu bar wants a monochrome template icon that the system tints.
    #[cfg(target_os = "macos")]
    {
        tray = tray
            .icon(tauri::image::Image::from_bytes(include_bytes!("../icons/tray-template.png"))?)
            .icon_as_template(true);
    }
    #[cfg(not(target_os = "macos"))]
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

/// The Mac's menu-bar menu, from `macmenu::host_menu`.
fn host_menu_items(app: &AppHandle, status: &StatusDto) -> tauri::Result<Menu<tauri::Wry>> {
    use tauri::menu::{CheckMenuItem, PredefinedMenuItem};
    let menu = Menu::new(app)?;
    for line in macmenu::host_menu(status, &system::snapshot(app)) {
        match line {
            macmenu::Line::Text(text) => menu.append(&MenuItem::new(app, text, false, None::<&str>)?)?,
            macmenu::Line::Action { id, label, enabled } => {
                menu.append(&MenuItem::with_id(app, id, label, enabled, None::<&str>)?)?
            }
            macmenu::Line::Check { id, label, checked } => {
                menu.append(&CheckMenuItem::with_id(app, id, label, true, checked, None::<&str>)?)?
            }
            macmenu::Line::Separator => menu.append(&PredefinedMenuItem::separator(app)?)?,
        }
    }
    Ok(menu)
}

/// The PC's tray menu.
fn client_menu(app: &AppHandle, status: &StatusDto) -> tauri::Result<Menu<tauri::Wry>> {
    use tauri::menu::{CheckMenuItem, PredefinedMenuItem};
    let idle = matches!(status.state, "disconnected" | "failed");
    let open = MenuItem::with_id(app, "open", "Open Sidescreen", true, None::<&str>)?;
    let toggle = if idle {
        MenuItem::with_id(app, "connect", "Connect", status.paired || status.peer_host.is_some(), None::<&str>)?
    } else {
        MenuItem::with_id(app, "disconnect", "Disconnect", true, None::<&str>)?
    };
    let pause = CheckMenuItem::with_id(app, "pause", "Pause sharing", true, status.sharing_paused, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Sidescreen", true, None::<&str>)?;
    let line = PredefinedMenuItem::separator(app)?;
    let line2 = PredefinedMenuItem::separator(app)?;
    Menu::with_items(app, &[&open, &line, &toggle, &pause, &line2, &quit])
}

/// The last menu shown, so it is only replaced when something in it changed: replacing an
/// open menu closes it (it closed by itself every few seconds on the Mac, 2026-11-19).
static LAST_MENU: Mutex<Option<String>> = Mutex::new(None);

fn refresh_tray_menu(app: &AppHandle, status: &StatusDto) {
    let key = if ROLE == "host" {
        format!("{:?}", macmenu::host_menu(status, &system::snapshot(app)))
    } else {
        format!("{} {} {} {}", status.state, status.paired, status.peer_host.is_some(), status.sharing_paused)
    };
    let mut last = LAST_MENU.lock().unwrap();
    if last.as_deref() == Some(key.as_str()) {
        return;
    }
    let menu = if ROLE == "host" { host_menu_items(app, status) } else { client_menu(app, status) };
    if let (Some(tray), Ok(menu)) = (app.tray_by_id("main"), menu) {
        let _ = tray.set_menu(Some(menu));
        *last = Some(key);
    }
}


/// Stops the session and the engines before the process goes away, so the Mac is never left
/// with a virtual display or a running stream host.
fn wind_down(app: &AppHandle) {
    let shared = app.state::<Shared>().inner().clone();
    let background = app.state::<Background>();
    shared.cancel_session();
    background.shutdown.cancel();
    let task = background.task.lock().unwrap().take();
    let mouse = app.state::<Mouse>().inner().clone();
    tauri::async_runtime::block_on(async move {
        let _ = tokio::time::timeout(EXIT_WAIT, async { mouse.lock().await.stop().await }).await;
        let _ = tokio::time::timeout(EXIT_WAIT, async {
            if let Some(task) = task {
                let _ = task.await;
            }
            while shared.has_session() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await;
    });
}

pub fn run() {
    let path = Config::default_path();
    let shared: Shared = Arc::new(match Config::load(&path) {
        Ok(cfg) => {
            // Make sure the file exists so the device ID stays the same across launches.
            let _ = cfg.save(&path);
            AppState::new(cfg, path, None)
        }
        Err(e) => AppState::new(Config::default_new(), path, Some(e.to_string())),
    });

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, None))
        .manage(shared.clone())
        .manage(Background { shutdown: CancellationToken::new(), task: Mutex::new(None) })
        .manage::<Mouse>(Arc::new(tokio::sync::Mutex::new(MouseManager::new())))
        .invoke_handler(tauri::generate_handler![
            get_status,
            connect,
            disconnect,
            find_mac,
            set_peer_host,
            save_settings,
            forget_peer,
            reset_config,
            open_privacy_settings,
            set_mouse_sharing,
            set_sharing_paused,
            set_close_to_tray,
            set_clipboard_sharing,
            set_language_follow,
            get_monitors,
            close_window,
            set_mac_side,
            bar::bar_expand,
            bar::bar_settings,
            bar::bar_windowed,
            bar::bar_move,
            system::get_system,
            system::set_start_at_login,
            system::install_missing
        ])
        .on_window_event(|window, event| {
            // Closing the window keeps the app in the tray, unless switched off in settings.
            if let WindowEvent::CloseRequested { api, .. } = event {
                let keep = window.app_handle().state::<Shared>().cfg.lock().unwrap().close_to_tray;
                if window.label() == "main" && !keep {
                    window.app_handle().exit(0);
                    return;
                }
                // The bar is the app's to show and hide (Alt+F4 on it must not destroy it).
                if window.label() == bar::LABEL {
                    api.prevent_close();
                    return;
                }
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .setup(move |app| {
            let handle = app.handle().clone();
            build_tray(&handle, &shared)?;

            // Push every state change to the window and the tray.
            let (watch_app, watch_shared) = (handle.clone(), shared.clone());
            tauri::async_runtime::spawn(async move {
                let mut rx = watch_shared.state.subscribe();
                while rx.changed().await.is_ok() {
                    announce(&watch_app, &watch_shared);
                }
            });

            // Keep the window and tray up to date with the mouse-sharing engine.
            let (mouse_app, mouse_shared) = (handle.clone(), shared.clone());
            let mouse = app.state::<Mouse>().inner().clone();
            tauri::async_runtime::spawn(async move {
                let mut rx = mouse.lock().await.subscribe();
                while rx.changed().await.is_ok() {
                    let status = rx.borrow().clone();
                    mouse_shared.set_mouse_status(status);
                    announce(&mouse_app, &mouse_shared);
                }
            });
            let mouse_app = handle.clone();
            tauri::async_runtime::spawn(async move { sync_mouse(&mouse_app).await });

            // First launch: start with the computer, so sharing is there without thinking.
            if shared.take_first_run() {
                use tauri_plugin_autostart::ManagerExt;
                let _ = handle.autolaunch().enable();
            }

            #[cfg(windows)]
            {
                bar::create(&handle)?;
                tauri::async_runtime::spawn(bar::keep_placed(handle.clone()));
            }

            #[cfg(target_os = "macos")]
            {
                // A menu-bar app: no Dock icon and no window, except the setup guide while
                // something it needs is still missing.
                app.set_activation_policy(tauri::ActivationPolicy::Accessory);
                if let Some(window) = app.get_webview_window("main") {
                    let sys = system::snapshot(&handle);
                    let ready = sys.screen_recording == Some(true) && sys.accessibility == Some(true) && sys.missing.is_none();
                    if ready {
                        let _ = window.hide();
                    } else {
                        let _ = window.set_size(tauri::LogicalSize::new(460.0, 560.0));
                        let _ = window.center();
                        let _ = handle.show();
                        let _ = window.set_focus();
                    }
                }
                // Permissions and start-at-login change outside the app; keep the menu true.
                let (menu_app, menu_shared) = (handle.clone(), shared.clone());
                tauri::async_runtime::spawn(async move {
                    loop {
                        tokio::time::sleep(Duration::from_secs(3)).await;
                        refresh_tray_menu(&menu_app, &menu_shared.status());
                    }
                });
                let background = app.state::<Background>();
                let task = tauri::async_runtime::spawn(host::run(
                    handle.clone(),
                    shared.clone(),
                    background.shutdown.clone(),
                ));
                *background.task.lock().unwrap() = Some(task);
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building the Sidescreen app");

    app.run(|app, event| {
        if let RunEvent::Exit = event {
            wind_down(app);
        }
    });
}
