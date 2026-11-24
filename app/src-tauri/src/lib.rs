//! The Sidescreen app: a small window and tray icon over `sidescreen_core`.
//! Windows builds are the client (the PC that shows the display); macOS builds are the host.

mod appearance;
mod awake;
mod bar;
mod display;
pub mod state;
mod system;
mod macmenu;
// The Mac's native menu-bar panel; only its pure part is used elsewhere (in tests).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod panel;

use serde::{Deserialize, Serialize};
use sidescreen_core::config::{Config, Peer, Placement, StreamSettings, WindowMode};
use sidescreen_core::side::Side;
use sidescreen_core::mouse::MouseManager;
use sidescreen_core::session::State;
use state::{AppState, PairingDto, StatusDto, ROLE};
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
    monitor: Option<u32>,
    /// The user picked `monitor` with this change (a monitor button), rather than it being
    /// carried along with another setting. Read only where the display window is (Windows).
    #[serde(default)]
    #[cfg_attr(not(windows), allow(dead_code))]
    monitor_picked: bool,
}

#[derive(Debug, Serialize)]
struct FoundDto {
    name: String,
    host: String,
    id: String,
    /// The Mac is showing its pairing code right now.
    pairing: bool,
}

impl From<sidescreen_core::discovery::Found> for FoundDto {
    fn from(f: sidescreen_core::discovery::Found) -> FoundDto {
        FoundDto { name: f.name, host: f.host, id: f.id, pairing: f.pairing }
    }
}

#[tauri::command]
fn get_status(shared: tauri::State<'_, Shared>) -> StatusDto {
    shared.status()
}

/// Starts the display; it needs the connection to the Mac to be up.
fn start_display(app: &AppHandle, shared: &Shared) -> Result<(), String> {
    if shared.status().connection != "connected" {
        return Err("connect to the Mac first".to_string());
    }
    client::start_session(app, shared.clone())
}

#[tauri::command]
fn display_start(app: AppHandle, shared: tauri::State<'_, Shared>) -> Result<(), String> {
    start_display(&app, shared.inner())
}

#[tauri::command]
fn display_stop(app: AppHandle, shared: tauri::State<'_, Shared>) {
    // The display window goes at once; the session winds down behind it.
    #[cfg(windows)]
    display::close(&app);
    #[cfg(not(windows))]
    let _ = app;
    shared.cancel_session();
}

/// Until the window is rebuilt this is `display_start`.
#[tauri::command]
fn connect(app: AppHandle, shared: tauri::State<'_, Shared>) -> Result<(), String> {
    start_display(&app, shared.inner())
}

/// Until the window is rebuilt this is `display_stop`.
#[tauri::command]
fn disconnect(shared: tauri::State<'_, Shared>) {
    shared.cancel_session();
}

/// Brings the connection to the Mac up: the chosen one (or the current one), pairing with `code`
/// first when one is typed or there is no pairing yet. The display is not started.
async fn connect_to(app: &AppHandle, shared: &Shared, key: Option<String>, code: Option<String>) -> Result<(), String> {
    shared.connect_to(key.as_deref(), code.as_deref(), sidescreen_core::link::CONTROL_PORT).await?;
    asked_to_connect(app, shared).await;
    Ok(())
}

/// The user asked to connect (Connect, a new Mac, or the Mac asking): the connection comes up,
/// and with it the display when the user wants that. Already up, that is decided now.
async fn asked_to_connect(app: &AppHandle, shared: &Shared) {
    shared.ask_connect(true);
    sync_mouse(app).await;
    if ROLE == "client" && shared.starts_display_now(&shared.mouse_status()) {
        if let Err(e) = start_display(app, shared) {
            eprintln!("sidescreen: the display did not start on connect: {e}");
        }
    }
}

/// The PC's door for "please connect" from a paired Mac: a Mac that proves it holds the pairing
/// token brings the connection up (switching from another Mac if need be). When the port is
/// taken it is logged and tried again every 10 seconds.
#[cfg(windows)]
async fn listen_for_connect_requests(app: AppHandle, shared: Shared) {
    use sidescreen_core::link::request::{serve_requests, REQUEST_PORT};
    loop {
        let token_shared = shared.clone();
        let (ok_app, ok_shared) = (app.clone(), shared.clone());
        let result = serve_requests(
            std::net::SocketAddr::from(([0, 0, 0, 0], REQUEST_PORT)),
            move |mac_id| {
                let cfg = token_shared.cfg.lock().unwrap();
                cfg.peers.iter().find(|p| !p.id.is_empty() && p.id == mac_id && !p.token.is_empty()).map(|p| p.token.clone())
            },
            move |mac_id| {
                let (app, shared) = (ok_app.clone(), ok_shared.clone());
                // The listener must not wait for the connection to come up.
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = connect_to(&app, &shared, Some(mac_id), None).await {
                        eprintln!("the Mac asked to connect, but: {e}");
                    }
                });
            },
        )
        .await;
        if let Err(e) = result {
            eprintln!("cannot listen for connect requests on port {REQUEST_PORT}: {e}");
        }
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
}

#[tauri::command]
async fn connect_mac(
    app: AppHandle,
    shared: tauri::State<'_, Shared>,
    key: Option<String>,
    code: Option<String>,
) -> Result<(), String> {
    connect_to(&app, shared.inner(), key, code).await
}

/// Re-runs the connection to the current Mac without disconnecting: the sharing link is stopped
/// and brought up again, and a running display (on its own link) is left alone. The Mac keeps
/// the display for this PC meanwhile (`AppState::may_start_display`).
#[tauri::command]
async fn reconnect_mac(app: AppHandle, shared: tauri::State<'_, Shared>) -> Result<(), String> {
    // Checked first (a Mac is chosen and paired), so a refusal leaves the link as it was; this
    // also keeps the connection wanted, so the restart below brings it back up.
    shared.connect_to(None, None, sidescreen_core::link::CONTROL_PORT).await?;
    stop_mouse(&app).await;
    sync_mouse(&app).await;
    Ok(())
}

/// Gives a saved Mac a new address; the pairing stays. A connection to it that is wanted is
/// re-established, since its addresses (its identity to the connection) changed. A refused
/// address stops nothing. Returns the Mac's key now (a Mac known by address alone is keyed by
/// it), which the window then connects to.
#[tauri::command]
async fn set_mac_address(app: AppHandle, shared: tauri::State<'_, Shared>, key: String, host: String) -> Result<String, String> {
    let (changed, key) = shared.change_mac_address(&key, &host).await?;
    match changed {
        Some(true) if shared.want_connection() => sync_mouse(&app).await,
        Some(_) => announce(&app, &shared),
        None => {}
    }
    Ok(key)
}

#[tauri::command]
fn backdrop() -> &'static str {
    appearance::backdrop()
}

#[derive(Serialize)]
struct UpdateDto {
    current: String,
    latest: Option<String>,
    /// The release page of `latest`.
    url: String,
    /// `install_update` installs `latest` in place (Windows); otherwise the release page has it.
    in_place: bool,
}

/// The GitHub page of a release ("1.2.0" or "v1.2.0").
fn release_page(version: &str) -> String {
    format!("{OPENABLE_PREFIX}releases/tag/v{}", version.trim().trim_start_matches('v'))
}

/// Parses "v1.2.3" or "1.2.3" into numbers (a pre-release suffix is ignored).
#[cfg(not(windows))]
fn parse_version(tag: &str) -> Option<(u64, u64, u64)> {
    let core = tag.trim().trim_start_matches('v').split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    Some((parts.next()??, parts.next()??, parts.next()??))
}

#[cfg(not(windows))]
#[derive(Deserialize)]
struct Release {
    tag_name: String,
}

/// The Mac looks at the latest GitHub release; its update is downloaded from the release page.
#[cfg(not(windows))]
#[tauri::command]
async fn check_update() -> Result<UpdateDto, String> {
    let current = env!("CARGO_PKG_VERSION").to_string();
    let release: Release = reqwest::Client::builder()
        .user_agent("Sidescreen")
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?
        .get("https://api.github.com/repos/argaman123/sidescreen/releases/latest")
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    let newer = matches!((parse_version(&release.tag_name), parse_version(&current)), (Some(l), Some(c)) if l > c);
    let latest = newer.then(|| release.tag_name.trim_start_matches('v').to_string());
    Ok(UpdateDto { url: latest.as_deref().map(release_page).unwrap_or_default(), latest, current, in_place: false })
}

#[cfg(not(windows))]
#[tauri::command]
async fn install_update() -> Result<(), String> {
    Err("Updates come from the release page".into())
}

/// How long the PC waits for the update manifest.
#[cfg(windows)]
const CHECK_TIMEOUT: Duration = Duration::from_secs(10);
/// How long the PC waits for the manifest and the installer together.
#[cfg(windows)]
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);

/// A latest release without `latest.json` (a Mac-only release, say) has nothing for the PC: the
/// updater reports that as `ReleaseNotFound` (the endpoint did not answer 2xx).
#[cfg(windows)]
fn no_manifest_is_no_update<T>(checked: Result<Option<T>, tauri_plugin_updater::Error>) -> Result<Option<T>, String> {
    match checked {
        Err(tauri_plugin_updater::Error::ReleaseNotFound) => Ok(None),
        other => other.map_err(|e| e.to_string()),
    }
}

/// The PC reads the signed release's `latest.json` (the updater's endpoint in tauri.conf.json).
#[cfg(windows)]
#[tauri::command]
async fn check_update(app: AppHandle) -> Result<UpdateDto, String> {
    use tauri_plugin_updater::UpdaterExt;
    let update = no_manifest_is_no_update(
        app.updater_builder().timeout(CHECK_TIMEOUT).build().map_err(|e| e.to_string())?.check().await,
    )?;
    let latest = update.map(|u| u.version);
    Ok(UpdateDto {
        url: latest.as_deref().map(release_page).unwrap_or_default(),
        latest,
        current: app.package_info().version.to_string(),
        in_place: true,
    })
}

/// Downloads the newer release, checks its signature, and hands over to its installer, which
/// updates in place and starts the app again. Returns only if that fails.
#[cfg(windows)]
#[tauri::command]
async fn install_update(app: AppHandle) -> Result<(), String> {
    use std::sync::atomic::{AtomicBool, Ordering};
    use tauri_plugin_updater::UpdaterExt;
    let exiting = app.clone();
    let wound_down = Arc::new(AtomicBool::new(false));
    let marked = wound_down.clone();
    let built = app
        .updater_builder()
        .timeout(DOWNLOAD_TIMEOUT)
        // The updater ends the process itself (no `RunEvent::Exit`), so wind down first: the
        // display, the mouse and keyboard hooks, and the link to the Mac. This replaces the
        // plugin's own hook, so Tauri's cleanup (the tray icon, among others) is called here too.
        .on_before_exit(move || {
            marked.store(true, Ordering::SeqCst);
            wind_down(&exiting);
            exiting.cleanup_before_exit();
        })
        .build()
        .map_err(|e| e.to_string())?;
    let update = no_manifest_is_no_update(built.check().await)?.ok_or("Already up to date")?;
    let bytes = update.download(|_, _| {}, || {}).await.map_err(|e| e.to_string())?;
    // `wind_down` blocks on the async runtime, so the install runs off its worker threads.
    let installed = tauri::async_runtime::spawn_blocking(move || update.install(bytes))
        .await
        .map_err(|e| e.to_string())
        .and_then(|r| r.map_err(|e| e.to_string()));
    // The installer did not start (an antivirus, say) after the app had already wound down:
    // start again rather than run on half stopped.
    if installed.is_err() && wound_down.load(Ordering::SeqCst) {
        app.request_restart();
    }
    installed
}

const OPENABLE_PREFIX: &str = "https://github.com/argaman123/sidescreen/";

/// Only the project's own GitHub pages may be handed to the system browser.
fn url_is_openable(url: &str) -> bool {
    url.starts_with(OPENABLE_PREFIX) && !url.contains("/..") && !url.chars().any(|c| c.is_whitespace() || c.is_control() || c == '"' || c == '^' || c == '&')
}

#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    if !url_is_openable(&url) {
        return Err("That address cannot be opened".into());
    }
    #[cfg(windows)]
    let mut cmd = {
        use std::os::windows::process::CommandExt;
        let mut c = std::process::Command::new("rundll32");
        c.args(["url.dll,FileProtocolHandler", &url]).creation_flags(0x0800_0000); // no console window
        c
    };
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = std::process::Command::new("open");
        c.arg(&url);
        c
    };
    #[cfg(not(any(windows, target_os = "macos")))]
    let mut cmd = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(&url);
        c
    };
    cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
}

#[tauri::command]
async fn add_mac_by_address(app: AppHandle, shared: tauri::State<'_, Shared>, host: String, code: String) -> Result<(), String> {
    // Saved only once it has paired.
    shared.add_mac_by_address(&host, &code, sidescreen_core::link::CONTROL_PORT).await?;
    asked_to_connect(&app, shared.inner()).await;
    Ok(())
}

#[tauri::command]
async fn disconnect_mac(app: AppHandle, shared: tauri::State<'_, Shared>) -> Result<(), String> {
    disconnect_all(&app, shared.inner()).await;
    Ok(())
}

/// Ends the whole connection to the Mac (the display with it) until the user connects again.
async fn disconnect_all(app: &AppHandle, shared: &Shared) {
    shared.set_want_connection(false);
    shared.ask_connect(false);
    shared.cancel_session();
    // Stopping the engine tells the Mac goodbye.
    stop_mouse(app).await;
}

#[tauri::command]
async fn set_auto_connect(app: AppHandle, shared: tauri::State<'_, Shared>, enabled: bool) -> Result<(), String> {
    shared.set_auto_connect(enabled)?;
    announce(&app, &shared);
    Ok(())
}

#[tauri::command]
async fn forget_mac(app: AppHandle, shared: tauri::State<'_, Shared>, key: String) -> Result<(), String> {
    let was_current = shared.cfg.lock().unwrap().current.as_deref() == Some(key.as_str());
    if was_current {
        shared.cancel_session();
    }
    shared.forget_mac(&key)?;
    sync_mouse(&app).await;
    Ok(())
}

#[tauri::command]
async fn find_mac() -> Option<FoundDto> {
    sidescreen_core::discovery::find_first(Duration::from_secs(3)).await.map(FoundDto::from)
}

/// Every Sidescreen Mac on the network, once each, and whether it is showing its pairing code.
#[tauri::command]
async fn find_macs() -> Vec<FoundDto> {
    sidescreen_core::discovery::find_all(Duration::from_secs(3)).await.into_iter().map(FoundDto::from).collect()
}

// ---- the Mac: pairing mode and the PCs it knows ----

/// Opens pairing mode: the Mac shows a code that a PC types to pair, for two minutes or until
/// one PC has.
#[tauri::command]
fn start_pairing(app: AppHandle, shared: tauri::State<'_, Shared>) -> Result<PairingDto, String> {
    if ROLE != "host" {
        return Err("pairing is started on the Mac".to_string());
    }
    shared.start_pairing();
    announce(&app, &shared);
    let (code, seconds_left) = shared.pairing_open().ok_or("pairing mode did not open")?;
    Ok(PairingDto { code, seconds_left })
}

#[tauri::command]
fn cancel_pairing(app: AppHandle, shared: tauri::State<'_, Shared>) {
    shared.cancel_pairing();
    announce(&app, &shared);
}

/// Ends the connection of `key` (or, with none, of whichever PC is connected, or being
/// restarted): the sharing channel is told to disconnect it and its display connection is cut.
/// A PC in the middle of a restart is cut again the moment it is back, so a Disconnect pressed
/// in the restart's gap is not lost.
fn cut_pc(shared: &AppState, key: Option<&str>) {
    let now = std::time::Instant::now();
    let connected = shared.connected_pc();
    let Some(target) = key.map(str::to_string).or_else(|| connected.clone()).or_else(|| shared.restarting_pc(now)) else {
        return;
    };
    shared.kick_control(&target);
    shared.cut_when_back(&target, now);
    #[cfg(target_os = "macos")]
    if connected.as_deref() == Some(target.as_str()) {
        let _ = shared.mac_commands.1.send(sidescreen_core::share::mac::MacCommand::Disconnect);
    }
}

/// Lets a PC in again, or blocks it. Blocking the connected PC disconnects it.
#[tauri::command]
async fn set_pc_allowed(app: AppHandle, shared: tauri::State<'_, Shared>, key: String, allowed: bool) -> Result<(), String> {
    shared.set_pc_allowed(&key, allowed)?;
    if !allowed {
        cut_pc(&shared, Some(&key));
    }
    sync_mouse(&app).await;
    Ok(())
}

/// Disconnects the PC that is connected.
#[tauri::command]
fn disconnect_pc(shared: tauri::State<'_, Shared>) {
    cut_pc(&shared, None);
}

/// Re-runs the connected PC's connection (Mac only): the sharing link is dropped without the
/// sticky disconnect, so the PC takes it for a lost network and connects again. Its display, on
/// the control link, is never cut (unlike `cut_pc`).
#[tauri::command]
fn restart_pc(shared: tauri::State<'_, Shared>) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let Some(pc) = shared.connected_pc() else {
            return Err("no PC is connected".to_string());
        };
        shared
            .mac_commands
            .1
            .send(sidescreen_core::share::mac::MacCommand::Restart)
            .map_err(|_| "mouse sharing is not running".to_string())?;
        shared.note_restart(&pc, std::time::Instant::now());
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = shared;
        Err("only the Mac restarts a PC's connection".to_string())
    }
}

/// Asks a paired PC to connect to this Mac. Another PC that is connected is disconnected first:
/// the user asked for this one.
#[tauri::command]
async fn connect_pc(shared: tauri::State<'_, Shared>, key: String) -> Result<(), String> {
    let (pc, my_id) = {
        let cfg = shared.cfg.lock().unwrap();
        (cfg.peer(&key).cloned().ok_or("that PC is not in the list")?, cfg.device_id.clone())
    };
    if pc.token.is_empty() {
        return Err("that PC is not paired".to_string());
    }
    if !pc.allowed {
        return Err("that PC is blocked".to_string());
    }
    let addrs = pc.candidates();
    if addrs.is_empty() {
        return Err("the saved address of that PC is not valid".to_string());
    }
    // Everything about the target is checked: only then is the current PC let go.
    match shared.connected_pc() {
        Some(id) if id == pc.id => return Ok(()),
        Some(_) => cut_pc(&shared, None),
        None => {}
    }
    match sidescreen_core::link::request::ask_to_connect(&addrs, &my_id, &pc.token).await {
        Ok(true) => Ok(()),
        Ok(false) => Err("that PC did not accept the request".to_string()),
        Err(e) => Err(format!("cannot reach that PC: {e}")),
    }
}

/// Removes a PC. The stream host's own pairing goes with the last one.
#[tauri::command]
async fn forget_pc(app: AppHandle, shared: tauri::State<'_, Shared>, key: String) -> Result<(), String> {
    cut_pc(&shared, Some(&key));
    if shared.forget_pc(&key)? {
        let _ = std::fs::remove_file(sunshine_dir(&shared).join("sunshine_state.json"));
    }
    sync_mouse(&app).await;
    Ok(())
}

#[tauri::command]
fn set_stay_awake(app: AppHandle, shared: tauri::State<'_, Shared>, enabled: bool) -> Result<(), String> {
    shared.set_stay_awake(enabled)?;
    announce(&app, &shared);
    Ok(())
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
    // A restart opens the display where it is now: a window the user dragged to another monitor
    // stays there, unless the change picks a monitor.
    #[cfg(windows)]
    let settings = if shared.has_session() {
        let monitor = display::restart_monitor(settings.monitor, settings.monitor_picked, display::windowed_monitor());
        SettingsInput { monitor, ..settings }
    } else {
        settings
    };
    shared.save_settings(StreamSettings {
        width: settings.width,
        height: settings.height,
        fps: settings.fps,
        bitrate_kbps: settings.bitrate_mbps.saturating_mul(1000),
        window_mode,
        monitor: settings.monitor,
    })?;
    // A running display restarts to pick up the new settings; the sharing link stays as it is.
    // Its window stays up, "Starting" in it, until the new session's viewer is in it.
    let mut restarted = Ok(());
    if shared.has_session() {
        #[cfg(windows)]
        let asked_at = display::restarting();
        shared.cancel_session();
        let mut waited = 0;
        while shared.has_session() {
            if waited >= 4000 {
                restarted = Err("the display did not stop; the new settings are saved, start it again".to_string());
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            waited += 50;
        }
        // Closed while the old session wound down: the display stays closed.
        #[cfg(windows)]
        let closed = !display::restart_still_wanted(asked_at);
        #[cfg(not(windows))]
        let closed = false;
        if restarted.is_ok() && !closed {
            restarted = start_display(&app, shared.inner());
        }
        // Given up: the window held up for the restart goes.
        #[cfg(windows)]
        if restarted.is_err() {
            display::close(&app);
        }
    }
    // Sharing places the pointer in the viewer by the stream's shape; it hears of the new size
    // whether or not the display came back.
    sync_mouse(&app).await;
    restarted
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
    /// 1-based, left to right then top to bottom (what `StreamSettings.monitor` means).
    number: u32,
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
}

/// This PC's monitors, for the arrangement editor.
#[tauri::command]
fn get_monitors() -> Vec<MonitorDto> {
    #[cfg(windows)]
    return sidescreen_core::layout::numbered_monitors(&sidescreen_core::winwindow::monitors())
        .into_iter()
        .map(|(number, m)| MonitorDto { number, left: m.left, top: m.top, right: m.right, bottom: m.bottom })
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

/// PC: while the Mac has the keyboard, Ctrl reaches it as Command and Windows as Control. The
/// running connection follows at once.
#[tauri::command]
async fn set_ctrl_as_cmd(app: AppHandle, shared: tauri::State<'_, Shared>, enabled: bool) -> Result<(), String> {
    shared.set_ctrl_as_cmd(enabled)?;
    sync_mouse(&app).await;
    Ok(())
}

/// PC: the wheel scrolls the Mac the other way round. The running connection follows at once;
/// the display from its next start.
#[tauri::command]
async fn set_reverse_scroll(app: AppHandle, shared: tauri::State<'_, Shared>, enabled: bool) -> Result<(), String> {
    shared.set_reverse_scroll(enabled)?;
    sync_mouse(&app).await;
    Ok(())
}

/// PC: the display starts whenever the connection to the Mac comes up.
#[tauri::command]
fn set_start_display_on_connect(app: AppHandle, shared: tauri::State<'_, Shared>, enabled: bool) -> Result<(), String> {
    shared.set_start_display_on_connect(enabled)?;
    announce(&app, &shared);
    Ok(())
}

#[tauri::command]
fn set_close_to_tray(app: AppHandle, shared: tauri::State<'_, Shared>, enabled: bool) -> Result<(), String> {
    shared.set_close_to_tray(enabled)?;
    announce(&app, &shared);
    Ok(())
}

#[tauri::command]
async fn set_mouse_sharing(app: AppHandle, shared: tauri::State<'_, Shared>, enabled: bool) -> Result<(), String> {
    shared.set_mouse_sharing(enabled)?;
    sync_mouse(&app).await;
    Ok(())
}

/// `side`: where the Mac sits relative to the PC; `monitor`: the PC monitor number it sits
/// below (only used with "bottom").
#[tauri::command]
async fn set_placement(
    app: AppHandle,
    shared: tauri::State<'_, Shared>,
    side: String,
    monitor: Option<u32>,
) -> Result<(), String> {
    let side = match side.as_str() {
        "left" => Side::Left,
        "right" => Side::Right,
        "top" => Side::Top,
        "bottom" => Side::Bottom,
        other => return Err(format!("unknown side: {other}")),
    };
    shared.set_placement(Placement { side, monitor })?;
    sync_mouse(&app).await;
    Ok(())
}

/// The current UI's name for `set_placement` without a monitor; goes with the redesign.
#[tauri::command]
async fn set_mac_side(app: AppHandle, shared: tauri::State<'_, Shared>, side: String) -> Result<(), String> {
    set_placement(app, shared, side, None).await
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
    match shared.connection_role() {
        Some(role) => manager.start(role).await,
        // A refusal stays on show until the user acts; stopping would wipe it.
        None if matches!(manager.status(), sidescreen_core::mouse::MouseStatus::Ended(_)) && !shared.want_connection() => {}
        None => manager.stop().await,
    }
    shared.set_mouse_status(manager.status());
    drop(manager);
    announce(app, &shared);
}

/// Takes the connection down (the Mac is told goodbye), whatever state it is in.
async fn stop_mouse(app: &AppHandle) {
    let shared = app.state::<Shared>().inner().clone();
    let mouse = app.state::<Mouse>().inner().clone();
    let mut manager = mouse.lock().await;
    manager.stop().await;
    shared.set_mouse_status(manager.status());
    drop(manager);
    announce(app, &shared);
}


fn sunshine_dir(shared: &AppState) -> std::path::PathBuf {
    shared.path.parent().unwrap_or(std::path::Path::new(".")).join("sunshine")
}

fn announce(app: &AppHandle, shared: &AppState) {
    // The Mac stays awake while a PC is connected (nothing happens on the PC).
    awake::set(|| shared.wants_awake());
    let status = shared.status();
    refresh_tray_menu(app, &status);
    #[cfg(target_os = "macos")]
    push_panel_status(app, shared, &status, false);
    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_tooltip(Some(state::tray_text(status.state, status.mouse_state)));
    }
    let _ = app.emit("state-changed", status);
}

/// Sends the Mac's panel the current state. Unless `always`, only when something in it changed.
#[cfg(target_os = "macos")]
fn push_panel(app: &AppHandle, shared: &AppState, always: bool) {
    push_panel_status(app, shared, &shared.status(), always);
}

#[cfg(target_os = "macos")]
fn push_panel_status(app: &AppHandle, shared: &AppState, status: &StatusDto, always: bool) {
    if !panel::running() {
        return;
    }
    let version = app.package_info().version.to_string();
    panel::push(&panel::panel_dto(status, &system::snapshot(app), shared.stay_awake(), &version), always);
}

/// The panel was given up (it kept exiting): the menu-bar menu takes its place.
#[cfg(target_os = "macos")]
fn fall_back_to_tray(app: &AppHandle) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if handle.tray_by_id("main").is_some() {
            return;
        }
        let shared = handle.state::<Shared>().inner().clone();
        if let Err(e) = build_tray(&handle, &shared) {
            eprintln!("sidescreen: the menu-bar menu could not be built: {e}");
        }
    });
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
        let peer = shared.cfg.lock().unwrap().current_peer().cloned().ok_or("choose a Mac first")?;
        // Everything the Mac can be reached at; whichever answers first is used.
        let ips: Vec<IpAddr> = peer.candidates();
        if ips.is_empty() {
            return Err("the saved Mac address is not valid".to_string());
        }
        let Some(cancel) = shared.begin_session() else {
            return Ok(());
        };
        let app = app.clone();
        // Not a failure from before any more: the display window shows "Starting" at once.
        shared.state.send_replace(State::Connecting);
        // The display window shows at once, where the session goes.
        #[cfg(windows)]
        let generation = display::begin(
            &app,
            peer.stream.window_mode != WindowMode::Windowed,
            peer.stream.monitor,
            peer.placement,
            (peer.stream.width, peer.stream.height),
        );
        // Not tracked by a handle: exit waits for `AppState`'s session slot to clear instead.
        tauri::async_runtime::spawn(async move {
            // The display is in use now: tell the Mac, on the same connection.
            sync_mouse(&app).await;
            let cfg = shared.cfg.lock().unwrap().clone();
            match RealViewer::new(peer.placement, peer.stream.monitor) {
                Ok(viewer) => {
                    // The viewer's window goes into the display window.
                    #[cfg(windows)]
                    let viewer = viewer.with_host(Arc::new(display::SessionHost { app: app.clone(), generation }));
                    let mut viewer = viewer;
                    let addrs: Vec<SocketAddr> = ips.iter().map(|ip| SocketAddr::new(*ip, CONTROL_PORT)).collect();
                    let pairing_app = app.clone();
                    let old_key = peer.key();
                    let old_key2 = old_key.clone();
                    let save_now = move |peer: Peer| {
                        let shared = pairing_app.state::<Shared>().inner().clone();
                        shared.write_back_pairing(&old_key, peer);
                        // A new pairing is what mouse sharing authenticates with.
                        let app = pairing_app.clone();
                        tauri::async_runtime::spawn(async move { sync_mouse(&app).await });
                    };
                    if let Some(peer) = run(addrs, &cfg, &mut viewer, &shared.state, cancel, &save_now, None).await {
                        // Only onto the entry if it is still there, and never changing which
                        // Mac is current (the user may have switched or forgotten it meanwhile).
                        shared.write_back_session(&old_key2, &peer);
                    }
                }
                Err(e) => {
                    shared.state.send_replace(State::Failed { part: "Moonlight".to_string(), reason: e.to_string() });
                }
            }
            // The window goes, unless it shows why the session failed, or a restart holds it.
            #[cfg(windows)]
            display::finish(&app, matches!(*shared.state.borrow(), State::Failed { .. }));
            shared.end_session();
            sync_mouse(&app).await;
            announce(&app, &shared);
        });
        Ok(())
    }
}

#[cfg(target_os = "macos")]
mod host {
    use super::*;
    use sidescreen_core::discovery;
    use sidescreen_core::link::{server::Server, CONTROL_PORT};
    use sidescreen_core::session::engines::{cleanup_orphans, RealHostEngines};
    use sidescreen_core::session::host::serve_gated;
    use sidescreen_core::session::HostEngines;
    use state::PairingGate;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// Keeps the Mac announced on the network, and says in the announcement whether its pairing
    /// code is showing. It follows pairing mode as it opens, closes and runs out.
    async fn announce_presence(app: AppHandle, shared: Shared, name: String, id: String) {
        let mut changes = shared.pairing_changes();
        let mut showing = shared.pairing_open().is_some();
        let mut advertisement = discovery::advertise(&name, &id, CONTROL_PORT, showing).ok();
        loop {
            // Wake when pairing mode is opened or closed, or when it runs out.
            let ends = shared.pairing_ends_in();
            tokio::select! {
                _ = changes.changed() => {}
                _ = async {
                    match ends {
                        Some(left) => tokio::time::sleep(left).await,
                        None => std::future::pending().await,
                    }
                } => {}
            }
            let open = shared.pairing_open().is_some();
            if open != showing {
                showing = open;
                // Drop the old announcement first: both use the same name.
                drop(advertisement.take());
                advertisement = discovery::advertise(&name, &id, CONTROL_PORT, showing).ok();
                announce(&app, &shared);
            }
        }
    }

    /// Listens for PCs and serves each one's display connection in its own task until the app
    /// shuts down.
    pub async fn run(app: AppHandle, shared: Shared, shutdown: CancellationToken) {
        let mut server = match Server::bind(([0, 0, 0, 0], CONTROL_PORT).into()).await {
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
        let presence = tokio::spawn(announce_presence(app.clone(), shared.clone(), name.clone(), id.clone()));
        let gate: Arc<dyn sidescreen_core::link::PairGate> = Arc::new(PairingGate(shared.clone()));
        let mut serving = tokio::task::JoinSet::new();

        // An earlier run that crashed or was force-quit may have left its engines behind.
        cleanup_orphans(&sunshine_dir(&shared)).await;

        loop {
            // The trusted peer is read when a PC actually connects, so Forget applies at once.
            let for_lookup = shared.clone();
            let trusted = move |pc_id: &str| state::known_pc(&for_lookup.cfg.lock().unwrap(), pc_id).filter(|p| p.allowed);
            // Each PC's handshake runs in its own task inside the server: a silent caller does
            // not hold up the others.
            let accepted = tokio::select! {
                _ = shutdown.cancelled() => break,
                r = server.accept_from(trusted, gate.clone(), (&name, &id)) => r,
            };
            while serving.try_join_next().is_some() {}
            let Ok((conn, new_peer, pc_id, remote)) = accepted else {
                continue;
            };
            if new_peer.is_none() {
                // A known PC proved itself from here: its id (if it was migrated from 1.0) and
                // its address now.
                shared.learn_pc(&pc_id);
                shared.saw_pc_at(&pc_id, remote.ip());
            }
            if let Some(peer) = new_peer {
                // The stream host's own pairing is never touched here: it goes with the last PC.
                if let Err(e) = shared.pair_pc(peer) {
                    eprintln!("sidescreen: a PC paired, but it could not be saved: {e}");
                }
                // Mouse sharing now answers this PC.
                let app = app.clone();
                tauri::async_runtime::spawn(async move { sync_mouse(&app).await });
            }
            let (user, pass) = {
                let cfg = shared.cfg.lock().unwrap();
                (cfg.sunshine_user.clone(), cfg.sunshine_pass.clone())
            };
            let (task_shared, task_shutdown, dir) = (shared.clone(), shutdown.clone(), sunshine_dir(&shared));
            serving.spawn(async move {
                let shared = task_shared;
                let (cut, seq) = shared.register_control(&pc_id);
                // The PC says which side of it this Mac sits on: where the virtual display goes.
                let arranged = (shared.clone(), pc_id.clone());
                let on_arrangement = move |mac_side: Side| {
                    let known = arranged.0.cfg.lock().unwrap().peer(&arranged.1).map(|p| p.placement.side);
                    if known != Some(mac_side) {
                        let _ = arranged.0.set_pc_side(&arranged.1, mac_side);
                    }
                };
                // Where the virtual display goes, asked when the stream starts (the PC announces
                // its arrangement first): toward this PC, opposite to where this Mac sits.
                let placed = (shared.clone(), pc_id.clone());
                let toward_pc = move || {
                    let cfg = placed.0.cfg.lock().unwrap();
                    cfg.peer(&placed.1).map(|p| p.placement.side.opposite()).unwrap_or(Side::Right)
                };
                let mut engines = state::SizeTracking { inner: RealHostEngines::new(dir, user, pass, toward_pc), state: &shared };
                let holds_display = AtomicBool::new(false);
                let may_start = || {
                    let yes = shared.may_start_display(&pc_id);
                    if yes {
                        holds_display.store(true, Ordering::SeqCst);
                    }
                    yes
                };
                let interrupted = tokio::select! {
                    _ = serve_gated(conn, &mut engines, &shared.state, &on_arrangement, &may_start) => false,
                    _ = cut.cancelled() => true,
                    _ = task_shutdown.cancelled() => true,
                };
                let held = holds_display.load(Ordering::SeqCst);
                if interrupted && held {
                    engines.stop().await;
                    shared.state.send_replace(State::Disconnected);
                }
                if held {
                    shared.display_released(&pc_id);
                }
                shared.unregister_control(&pc_id, seq);
            });
        }
        // Each task has seen the shutdown; give them a moment to stop their engines.
        presence.abort();
        while serving.join_next().await.is_some() {}
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
                // The Mac's menu offers this only while exactly one PC is paired.
                let (app, shared) = (app.clone(), for_menu.clone());
                tauri::async_runtime::spawn(async move {
                    let only = {
                        let cfg = shared.cfg.lock().unwrap();
                        let paired: Vec<_> = cfg.peers.iter().filter(|p| !p.token.is_empty()).map(|p| p.key()).collect();
                        (paired.len() == 1).then(|| paired[0].clone())
                    };
                    if let Some(key) = only {
                        cut_pc(&shared, Some(&key));
                        if shared.forget_pc(&key) == Ok(true) {
                            let _ = std::fs::remove_file(sunshine_dir(&shared).join("sunshine_state.json"));
                        }
                    }
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
                let (app, shared) = (app.clone(), for_menu.clone());
                tauri::async_runtime::spawn(async move {
                    let result = connect_to(&app, &shared, None, None).await;
                    tray_failed(&app, &shared, result);
                });
            }
            "disconnect" => {
                // The whole connection, not only the display.
                let (app, shared) = (app.clone(), for_menu.clone());
                tauri::async_runtime::spawn(async move { disconnect_all(&app, &shared).await });
            }
            "mouse" if ROLE == "client" => {
                // A second click while one is still being applied is ignored.
                static BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
                if BUSY.swap(true, std::sync::atomic::Ordering::SeqCst) {
                    return;
                }
                let (app, shared) = (app.clone(), for_menu.clone());
                tauri::async_runtime::spawn(async move {
                    let on = !shared.status().mouse_sharing;
                    let result = shared.set_mouse_sharing(on);
                    sync_mouse(&app).await;
                    tray_failed(&app, &shared, result);
                    BUSY.store(false, std::sync::atomic::Ordering::SeqCst);
                });
            }
            "display" if ROLE == "client" => {
                let (app, shared) = (app.clone(), for_menu.clone());
                let result = if matches!(shared.status().state, "connecting" | "connected") {
                    shared.cancel_session();
                    Ok(())
                } else {
                    start_display(&app, &shared)
                };
                tray_failed(&app, &shared, result);
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
            .icon(tauri::image::Image::from_bytes(include_bytes!("../icons/tray-template@2x.png"))?)
            .icon_as_template(true);
    }
    // The notification area shows the small icon size (16 px at 100%, 24 at 150%); the window
    // icon is the large one, which the shell would shrink and blur. icon.ico has the small
    // drawing at each of those sizes, so ask the app icon resource for the small one.
    #[cfg(windows)]
    {
        use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSMICON};
        let small = u32::try_from(unsafe { GetSystemMetrics(SM_CXSMICON) }).unwrap_or(0);
        let icon = (small > 0)
            .then(|| tauri::image::Image::from_app_icon_resource(small).ok())
            .flatten()
            .or_else(|| app.default_window_icon().cloned());
        if let Some(icon) = icon {
            tray = tray.icon(icon);
        }
    }
    #[cfg(not(any(target_os = "macos", windows)))]
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

/// A failed tray action opens the window, where the failure shows as a notice.
/// Either way the menu is rebuilt from the real status: a check item flips on click, and a
/// failure leaves the status (and so the rebuild key) unchanged.
fn tray_failed(app: &AppHandle, shared: &Shared, result: Result<(), String>) {
    *LAST_MENU.lock().unwrap() = None;
    refresh_tray_menu(app, &shared.status());
    if result.is_err() {
        show_window(app);
    }
}

/// The PC's tray menu.
fn client_menu(app: &AppHandle, status: &StatusDto) -> tauri::Result<Menu<tauri::Wry>> {
    use tauri::menu::{CheckMenuItem, IsMenuItem, PredefinedMenuItem};
    let open = MenuItem::with_id(app, "open", "Open Sidescreen", true, None::<&str>)?;
    let actions = state::client_tray_actions(status);
    let toggles = actions
        .iter()
        .filter_map(|a| a.checked.map(|on| CheckMenuItem::with_id(app, a.id, a.label, a.enabled, on, None::<&str>)))
        .collect::<tauri::Result<Vec<_>>>()?;
    let commands = actions
        .iter()
        .filter(|a| a.checked.is_none())
        .map(|a| MenuItem::with_id(app, a.id, a.label, a.enabled, None::<&str>))
        .collect::<tauri::Result<Vec<_>>>()?;
    let quit = MenuItem::with_id(app, "quit", "Quit Sidescreen", true, None::<&str>)?;
    let lines = [PredefinedMenuItem::separator(app)?, PredefinedMenuItem::separator(app)?, PredefinedMenuItem::separator(app)?];
    let mut items: Vec<&dyn IsMenuItem<tauri::Wry>> = vec![&open, &lines[0]];
    items.extend(toggles.iter().map(|t| t as &dyn IsMenuItem<tauri::Wry>));
    items.push(&lines[1]);
    items.extend(commands.iter().map(|c| c as &dyn IsMenuItem<tauri::Wry>));
    items.extend([&lines[2] as &dyn IsMenuItem<tauri::Wry>, &quit]);
    Menu::with_items(app, &items)
}

/// The last menu shown, so it is only replaced when something in it changed: replacing an
/// open menu closes it (it closed by itself every few seconds on the Mac, 2026-11-19).
static LAST_MENU: Mutex<Option<String>> = Mutex::new(None);

fn refresh_tray_menu(app: &AppHandle, status: &StatusDto) {
    // No tray (the Mac's panel stands in for it): nothing to build.
    if app.tray_by_id("main").is_none() {
        return;
    }
    let key = if ROLE == "host" {
        format!("{:?}", macmenu::host_menu(status, &system::snapshot(app)))
    } else {
        format!(
            "{} {} {} {} {}",
            status.state, status.paired, status.peer_host.is_some(), status.connection, status.mouse_sharing
        )
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
    // The panel's helper exits with its stdin closed, and is not restarted.
    #[cfg(target_os = "macos")]
    panel::stop();
    let shared = app.state::<Shared>().inner().clone();
    let background = app.state::<Background>();
    shared.cancel_session();
    background.shutdown.cancel();
    awake::set(|| false);
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
    // Only the running app keeps sharing.log; tests leave the owner's log alone.
    sidescreen_core::sharelog::enable();
    let path = Config::default_path();
    // The Mac does not trust the id a 1.0 settings file kept for its PC (see `Config::load_as`).
    let role = if ROLE == "host" { sidescreen_core::config::Role::Mac } else { sidescreen_core::config::Role::Pc };
    let shared: Shared = Arc::new(match Config::load_as(&path, role) {
        Ok(cfg) => {
            // Make sure the file exists so the device ID stays the same across launches.
            let _ = cfg.save(&path);
            AppState::new(cfg, path, None)
        }
        Err(e) => AppState::new(Config::default_new(), path, Some(e.to_string())),
    });

    let builder = tauri::Builder::default();
    // The PC updates in place; the Mac opens the release page.
    #[cfg(windows)]
    let builder = builder.plugin(tauri_plugin_updater::Builder::new().build());
    let app = builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, None))
        .manage(shared.clone())
        .manage(Background { shutdown: CancellationToken::new(), task: Mutex::new(None) })
        .manage::<Mouse>(Arc::new(tokio::sync::Mutex::new(MouseManager::new())))
        .invoke_handler(tauri::generate_handler![
            get_status,
            connect_mac,
            reconnect_mac,
            set_mac_address,
            backdrop,
            check_update,
            install_update,
            open_url,
            disconnect_mac,
            display_start,
            display_stop,
            set_auto_connect,
            forget_mac,
            add_mac_by_address,
            connect,
            disconnect,
            find_mac,
            find_macs,
            start_pairing,
            cancel_pairing,
            set_pc_allowed,
            disconnect_pc,
            restart_pc,
            connect_pc,
            forget_pc,
            set_stay_awake,
            set_peer_host,
            save_settings,
            forget_peer,
            reset_config,
            open_privacy_settings,
            set_mouse_sharing,
            set_close_to_tray,
            set_clipboard_sharing,
            set_language_follow,
            set_ctrl_as_cmd,
            set_reverse_scroll,
            set_start_display_on_connect,
            get_monitors,
            close_window,
            set_placement,
            set_mac_side,
            bar::bar_shape,
            bar::bar_settings,
            bar::bar_menu_open,
            display::display_full_screen,
            display::display_window,
            display::display_pull_down,
            display::display_minimize,
            system::get_system,
            system::set_start_at_login,
            system::install_missing
        ])
        .on_window_event(|window, event| {
            #[cfg(windows)]
            if window.label() == display::LABEL {
                display::on_event(window.app_handle(), event);
            }
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
                // Closing the display window (Alt+F4) closes the display.
                if window.label() == display::LABEL {
                    api.prevent_close();
                    #[cfg(windows)]
                    display::close(window.app_handle());
                    window.app_handle().state::<Shared>().cancel_session();
                    return;
                }
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .setup(move |app| {
            let handle = app.handle().clone();
            // The Mac's native panel when its helper is there; the tray's menu otherwise.
            #[cfg(target_os = "macos")]
            let panel_up = panel::start(&handle, fall_back_to_tray);
            #[cfg(not(target_os = "macos"))]
            let panel_up = false;
            if !panel_up {
                build_tray(&handle, &shared)?;
            }
            // The panel's first state, ready for when it starts reading.
            #[cfg(target_os = "macos")]
            push_panel(&handle, &shared, true);

            // Push every state change to the window and the tray.
            let (watch_app, watch_shared) = (handle.clone(), shared.clone());
            tauri::async_runtime::spawn(async move {
                let mut rx = watch_shared.state.subscribe();
                rx.mark_changed();
                while rx.changed().await.is_ok() {
                    announce(&watch_app, &watch_shared);
                }
            });

            // Keep the window and tray up to date with the mouse-sharing engine.
            let (mouse_app, mouse_shared) = (handle.clone(), shared.clone());
            let mouse = app.state::<Mouse>().inner().clone();
            tauri::async_runtime::spawn(async move {
                let mut rx = mouse.lock().await.subscribe();
                // The engine may have moved on (to Connected) before this subscription existed:
                // take the current value first, or the window keeps showing a stale state.
                rx.mark_changed();
                while rx.changed().await.is_ok() {
                    let status = rx.borrow().clone();
                    let back = match &status {
                        sidescreen_core::mouse::MouseStatus::Connected { mac_id, .. } => Some(mac_id.clone()),
                        _ => None,
                    };
                    // The display starts as a connection the user asked for comes up, when the user
                    // wants that.
                    let start = ROLE == "client" && mouse_shared.starts_display_now(&status);
                    // The Mac: the connected PC's Ctrl as ⌘, for typing into the display window
                    // (kept through a sharing reconnect).
                    #[cfg(target_os = "macos")]
                    if let Some(on) = sidescreen_core::keys_stream::setting_in(&status) {
                        sidescreen_core::keys_stream::follow_setting(on);
                    }
                    mouse_shared.set_mouse_status(status);
                    if start {
                        eprintln!("sidescreen: connected; the display starts (start the display when connected)");
                        if let Err(e) = start_display(&mouse_app, &mouse_shared) {
                            eprintln!("sidescreen: the display did not start on connect: {e}");
                        }
                    }
                    // A Disconnect pressed while this PC was restarting is carried out now.
                    if let Some(pc) = back {
                        if mouse_shared.back_from_restart(&pc, std::time::Instant::now()) {
                            cut_pc(&mouse_shared, Some(&pc));
                        }
                    }
                    announce(&mouse_app, &mouse_shared);
                }
            });
            let mouse_app = handle.clone();
            // Connecting at launch is the user's ask too (auto-connect).
            shared.ask_connect_at_launch();
            tauri::async_runtime::spawn(async move { sync_mouse(&mouse_app).await });

            // First launch: start with the computer, so sharing is there without thinking.
            if shared.take_first_run() {
                use tauri_plugin_autostart::ManagerExt;
                let _ = handle.autolaunch().enable();
            }

            #[cfg(windows)]
            {
                bar::create(&handle)?;
                display::create(&handle)?;
                // Waits, doing nothing, while no display is in use.
                tauri::async_runtime::spawn(display::keep_watch(handle.clone()));
                tauri::async_runtime::spawn(listen_for_connect_requests(handle.clone(), shared.clone()));
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
                // Permissions and start-at-login change outside the app; keep the menu (or the
                // panel) true.
                let (menu_app, menu_shared) = (handle.clone(), shared.clone());
                tauri::async_runtime::spawn(async move {
                    loop {
                        tokio::time::sleep(Duration::from_secs(3)).await;
                        let status = menu_shared.status();
                        refresh_tray_menu(&menu_app, &status);
                        push_panel_status(&menu_app, &menu_shared, &status, false);
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

#[cfg(test)]
mod open_url_tests {
    use super::url_is_openable;

    #[test]
    fn only_the_projects_github_pages_open() {
        assert!(url_is_openable("https://github.com/argaman123/sidescreen/releases/tag/v1.2.0"));
        assert!(!url_is_openable("https://github.com/argaman123/sidescreen"));
        assert!(!url_is_openable("https://github.com/argaman123/sidescreen-evil/x"));
        assert!(!url_is_openable("https://github.com/other/sidescreen/"));
        assert!(!url_is_openable("http://github.com/argaman123/sidescreen/"));
        assert!(!url_is_openable("https://evil.example/https://github.com/argaman123/sidescreen/"));
        assert!(!url_is_openable("https://github.com/argaman123/sidescreen/../x"));
        assert!(!url_is_openable("file:///C:/Windows/System32/calc.exe"));
        assert!(!url_is_openable("https://github.com/argaman123/sidescreen/ & calc"));
        assert!(!url_is_openable("https://github.com/argaman123/sidescreen/a&b"));
    }
}

#[cfg(test)]
mod update_tests {
    #[cfg(not(windows))]
    use super::parse_version;
    use super::{release_page, url_is_openable};

    #[test]
    fn a_versions_release_page_can_be_opened() {
        assert_eq!(release_page("1.2.0"), "https://github.com/argaman123/sidescreen/releases/tag/v1.2.0");
        assert_eq!(release_page("v1.2.0"), release_page("1.2.0"));
        assert!(url_is_openable(&release_page("1.2.0")));
    }

    #[cfg(windows)]
    #[test]
    fn a_release_without_latest_json_is_no_update() {
        use super::no_manifest_is_no_update;
        use tauri_plugin_updater::Error;
        assert_eq!(no_manifest_is_no_update::<()>(Err(Error::ReleaseNotFound)), Ok(None));
        assert_eq!(no_manifest_is_no_update(Ok(Some(()))), Ok(Some(())));
        assert_eq!(no_manifest_is_no_update::<()>(Ok(None)), Ok(None));
        assert!(no_manifest_is_no_update::<()>(Err(Error::UnsupportedOs)).is_err());
    }

    /// `install_update` runs the updater's `install` (and so `wind_down`, which blocks on the
    /// runtime) on a blocking thread; blocking on the runtime there must not panic.
    #[test]
    fn the_runtime_can_be_blocked_on_from_a_blocking_thread() {
        let inner = tauri::async_runtime::block_on(async {
            tauri::async_runtime::spawn_blocking(|| tauri::async_runtime::block_on(async { 7 })).await
        });
        assert_eq!(inner.unwrap(), 7);
    }

    #[cfg(not(windows))]
    #[test]
    fn release_tags_compare_as_versions() {
        assert_eq!(parse_version("v1.1.0"), Some((1, 1, 0)));
        assert_eq!(parse_version("1.10.2-beta"), Some((1, 10, 2)));
        assert!(parse_version("v1.10.0") > parse_version("v1.9.0"));
        assert_eq!(parse_version("latest"), None);
    }
}
