//! The viewer on the PC: Moonlight, launched with settings the app fixes.
//! Option names checked against moonlight-qt's commandlineparser.cpp on 2026-11-11.

use crate::config::{StreamSettings, WindowMode};
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use tokio::process::{Child, Command};

/// The app Sunshine always offers that streams the whole display.
const APP_NAME: &str = "Desktop";

pub fn find_exe() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    for var in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"] {
        if let Ok(dir) = std::env::var(var) {
            candidates.push(Path::new(&dir).join("Moonlight Game Streaming").join("Moonlight.exe"));
        }
    }
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            candidates.push(dir.join("Moonlight.exe"));
            candidates.push(dir.join("moonlight"));
        }
    }
    candidates.into_iter().find(|p| p.is_file())
}

pub fn stream_args(host: &str, s: &StreamSettings) -> Vec<String> {
    let mode = match s.window_mode {
        WindowMode::Windowed => "windowed",
        WindowMode::Borderless => "borderless",
    };
    [
        "stream", host, APP_NAME,
        "--resolution", &format!("{}x{}", s.width, s.height),
        "--fps", &s.fps.to_string(),
        "--bitrate", &s.bitrate_kbps.to_string(),
        "--display-mode", mode,
        "--absolute-mouse",
        "--capture-system-keys", "never",
    ]
    .iter()
    .map(|a| a.to_string())
    .collect()
}

pub fn pair_args(host: &str, pin: &str) -> Vec<String> {
    ["pair", host, "--pin", pin].iter().map(|a| a.to_string()).collect()
}

fn command(exe: &Path, args: Vec<String>) -> Command {
    let mut cmd = Command::new(exe);
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).kill_on_drop(true);
    cmd
}

/// A running viewer window.
pub struct Viewer {
    child: Child,
}

impl Viewer {
    pub fn spawn(exe: &Path, host: &str, s: &StreamSettings) -> std::io::Result<Viewer> {
        Ok(Viewer { child: command(exe, stream_args(host, s)).spawn()? })
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.id()
    }

    /// Resolves when the viewer window is closed.
    pub async fn wait(&mut self) -> std::io::Result<ExitStatus> {
        self.child.wait().await
    }

    pub async fn kill(&mut self) {
        let _ = self.child.kill().await;
    }
}

/// Runs Moonlight's pairing with a PIN we chose. True when it reports success.
pub async fn pair(exe: &Path, host: &str, pin: &str) -> std::io::Result<bool> {
    Ok(command(exe, pair_args(host, pin)).status().await?.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(mode: WindowMode) -> StreamSettings {
        StreamSettings { width: 2560, height: 1440, fps: 60, bitrate_kbps: 30000, window_mode: mode }
    }

    #[test]
    fn stream_args_windowed() {
        assert_eq!(
            stream_args("192.168.1.50", &settings(WindowMode::Windowed)),
            [
                "stream", "192.168.1.50", "Desktop",
                "--resolution", "2560x1440", "--fps", "60", "--bitrate", "30000",
                "--display-mode", "windowed",
                "--absolute-mouse", "--capture-system-keys", "never",
            ]
        );
    }

    #[test]
    fn stream_args_borderless() {
        let args = stream_args("h", &settings(WindowMode::Borderless));
        let i = args.iter().position(|a| a == "--display-mode").unwrap();
        assert_eq!(args[i + 1], "borderless");
    }

    #[test]
    fn pair_args_shape() {
        assert_eq!(pair_args("h", "1234"), ["pair", "h", "--pin", "1234"]);
    }
}
