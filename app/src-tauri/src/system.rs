//! What the window shows about this machine: a missing streaming app (with a way to install it),
//! starting at login, and on the Mac whether its permissions are on. Read fresh each time,
//! since all of it can change outside the app.

use serde::Serialize;
use tauri::AppHandle;
use tauri_plugin_autostart::ManagerExt;

#[derive(Debug, Clone, Serialize)]
pub struct SystemDto {
    /// The streaming app this machine needs but does not have.
    pub missing: Option<&'static str>,
    pub start_at_login: bool,
    /// Mac only: Screen Recording and Accessibility.
    pub screen_recording: Option<bool>,
    pub accessibility: Option<bool>,
}

/// The streaming app this machine needs: Moonlight shows the Mac on the PC, Sunshine sends it.
pub fn needed_app() -> &'static str {
    if cfg!(target_os = "macos") {
        "Sunshine"
    } else {
        "Moonlight"
    }
}

/// How the missing app is installed: the system's own package manager, in a window the user
/// can watch (it may ask for their password or approval).
pub fn install_command(app: &str) -> Option<(&'static str, Vec<String>)> {
    let words = |w: &[&str]| w.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    match app {
        "Moonlight" => Some((
            "cmd",
            words(&[
                "/C",
                "start",
                "Installing Moonlight",
                "winget",
                "install",
                "--id",
                "MoonlightGameStreamingProject.Moonlight",
                "-e",
                "--accept-source-agreements",
                "--accept-package-agreements",
            ]),
        )),
        "Sunshine" => Some((
            "osascript",
            words(&[
                "-e",
                // Homebrew first when the Mac does not have it (its installer asks for the password).
                "tell application \"Terminal\" to do script \"export PATH=/opt/homebrew/bin:/usr/local/bin:$PATH; command -v brew >/dev/null || /bin/bash -c \\\"$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)\\\"; export PATH=/opt/homebrew/bin:/usr/local/bin:$PATH; brew install lizardbyte/homebrew/sunshine-beta\"",
                "-e",
                "tell application \"Terminal\" to activate",
            ]),
        )),
        _ => None,
    }
}

fn is_missing() -> bool {
    if cfg!(target_os = "macos") {
        sidescreen_core::sunshine::find_exe().is_none()
    } else {
        sidescreen_core::moonlight::find_exe().is_none()
    }
}

#[cfg(target_os = "macos")]
fn permissions() -> (Option<bool>, Option<bool>) {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
    }
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> bool;
    }
    unsafe { (Some(CGPreflightScreenCaptureAccess()), Some(AXIsProcessTrusted())) }
}

#[cfg(not(target_os = "macos"))]
fn permissions() -> (Option<bool>, Option<bool>) {
    (None, None)
}

#[tauri::command]
pub fn get_system(app: AppHandle) -> SystemDto {
    snapshot(&app)
}

pub fn snapshot(app: &AppHandle) -> SystemDto {
    let (screen_recording, accessibility) = permissions();
    SystemDto {
        missing: is_missing().then(needed_app),
        start_at_login: app.autolaunch().is_enabled().unwrap_or(false),
        screen_recording,
        accessibility,
    }
}

#[tauri::command]
pub fn set_start_at_login(app: AppHandle, enabled: bool) -> Result<(), String> {
    let launcher = app.autolaunch();
    let result = if enabled { launcher.enable() } else { launcher.disable() };
    result.map_err(|e| format!("could not change starting at login: {e}"))
}

#[tauri::command]
pub fn install_missing() -> Result<(), String> {
    let (program, args) = install_command(needed_app()).ok_or("nothing to install")?;
    let child = std::process::Command::new(program).args(&args).spawn().map_err(|e| format!("could not start the installer: {e}"))?;
    reap_in_background(child);
    Ok(())
}

/// Waits for `child` on its own thread, so it is reaped when it ends (no zombie) without
/// holding anyone up.
fn reap_in_background(mut child: std::process::Child) -> std::thread::JoinHandle<std::io::Result<std::process::ExitStatus>> {
    std::thread::spawn(move || child.wait())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_started_installer_is_waited_for_in_the_background() {
        let child = if cfg!(windows) {
            std::process::Command::new("cmd").args(["/C", "exit 3"]).spawn()
        } else {
            std::process::Command::new("/bin/sh").args(["-c", "exit 3"]).spawn()
        }
        .unwrap();
        // Joined here only to see it: in the app nothing waits on it, yet it is reaped.
        let status = reap_in_background(child).join().unwrap().unwrap();
        assert_eq!(status.code(), Some(3));
    }

    #[cfg(windows)]
    #[test]
    fn moonlight_is_installed_with_winget_in_its_own_window() {
        let (program, args) = install_command("Moonlight").unwrap();
        assert_eq!(program, "cmd");
        let line = args.join(" ");
        assert!(line.contains("start"), "{line}");
        assert!(line.contains("winget install --id MoonlightGameStreamingProject.Moonlight -e"), "{line}");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn sunshine_is_installed_with_homebrew_in_terminal() {
        let (program, args) = install_command("Sunshine").unwrap();
        assert_eq!(program, "osascript");
        assert!(args.join(" ").contains("brew install lizardbyte/homebrew/sunshine-beta"));
    }

    #[test]
    fn nothing_else_has_an_installer() {
        assert!(install_command("Something else").is_none());
    }
}
