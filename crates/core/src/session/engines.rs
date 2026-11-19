//! The real engines behind the session traits.

use super::ViewerEngine;
use crate::config::StreamSettings;
use crate::moonlight::{self, Viewer};
use crate::side::Side;
use async_trait::async_trait;
use std::path::PathBuf;

/// Moonlight on the PC.
pub struct RealViewer {
    exe: PathBuf,
    viewer: Option<Viewer>,
    /// Which side of the PC the Mac sits on: the viewer opens on the monitor on that side.
    #[cfg_attr(not(windows), allow(dead_code))]
    mac_side: Side,
}

impl RealViewer {
    pub fn new(mac_side: Side) -> std::io::Result<RealViewer> {
        let exe = moonlight::find_exe().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "Moonlight is not installed on this PC")
        })?;
        Ok(RealViewer { exe, viewer: None, mac_side })
    }
}

#[async_trait]
impl ViewerEngine for RealViewer {
    async fn pair(&mut self, host: &str, pin: &str) -> std::io::Result<bool> {
        moonlight::pair(&self.exe, host, pin).await
    }

    async fn start(&mut self, host: &str, s: &StreamSettings) -> std::io::Result<()> {
        let viewer = Viewer::spawn(&self.exe, host, s)?;
        #[cfg(windows)]
        if let Some(pid) = viewer.pid() {
            let full_screen = s.window_mode != crate::config::WindowMode::Windowed;
            tokio::spawn(settle_viewer_window(pid, self.mac_side, full_screen));
        }
        self.viewer = Some(viewer);
        Ok(())
    }

    async fn closed(&mut self) {
        match self.viewer.as_mut() {
            Some(v) => {
                let _ = v.wait().await;
            }
            None => std::future::pending().await,
        }
    }

    async fn stop(&mut self) {
        if let Some(mut v) = self.viewer.take() {
            v.kill().await;
        }
    }
}

/// Gives the viewer window our title and icon, and puts it on the monitor next to the Mac.
/// The viewer sets its own title, and may move itself, a little after its window appears, so
/// this keeps at it for a while. It moves the window only once: after that it is the user's.
#[cfg(windows)]
async fn settle_viewer_window(pid: u32, mac_side: Side, full_screen: bool) {
    use crate::layout::{is_on, monitor_toward, viewer_frame};
    use crate::winwindow;
    let mut placed = false;
    for _ in 0..30 {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let Some(window) = winwindow::find_window_of_pid(pid) else { continue };
        winwindow::brand_window(window, VIEWER_TITLE);
        if placed {
            continue;
        }
        let (Some(monitor), Some(current)) =
            (monitor_toward(&winwindow::monitors(), mac_side), winwindow::window_rect(window))
        else {
            continue;
        };
        if !is_on(current, monitor) || (full_screen && current != monitor) {
            winwindow::move_window(window, viewer_frame(monitor, current, full_screen));
        }
        placed = true;
    }
}

#[cfg(windows)]
const VIEWER_TITLE: &str = "Sidescreen";

#[cfg(target_os = "macos")]
pub use mac::{cleanup_orphans, RealHostEngines};

#[cfg(target_os = "macos")]
mod mac {
    use crate::macdisplay;
    use crate::session::HostEngines;
    use crate::sunshine::{self, EngineError, Host};
    use async_trait::async_trait;
    use std::path::{Path, PathBuf};
    use std::process::Stdio;
    use std::time::Duration;

    const SUNSHINE_WAIT: Duration = Duration::from_secs(20);

    async fn quiet(program: &str, args: &[&str]) -> bool {
        tokio::process::Command::new(program)
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .is_ok_and(|s| s.success())
    }

    /// Stops engines left behind by an earlier run that crashed or was force-quit: a Sunshine
    /// started with our private config, and our virtual display helper. Never touches a Sunshine
    /// the user started themselves.
    pub async fn cleanup_orphans(data_dir: &Path) {
        let conf = sunshine::conf_path(data_dir);
        let conf = conf.to_string_lossy();
        // A Sunshine left over from long ago was seen ignoring a normal stop (2026-11-18).
        if quiet("pkill", &["-f", &conf]).await {
            tokio::time::sleep(Duration::from_secs(1)).await;
            quiet("pkill", &["-9", "-f", &conf]).await;
        }
        // By full path: process names are cut to 16 characters, which "sidescreen-display" is not.
        quiet("pkill", &["-f", "/sidescreen-display "]).await;
    }

    /// The virtual display and Sunshine on the Mac.
    pub struct RealHostEngines {
        data_dir: PathBuf,
        user: String,
        pass: String,
        host: Option<Host>,
        display: Option<macdisplay::VirtualDisplay>,
        pin_attempts: u32,
    }

    impl RealHostEngines {
        pub fn new(data_dir: PathBuf, user: String, pass: String) -> RealHostEngines {
            RealHostEngines { data_dir, user, pass, host: None, display: None, pin_attempts: 0 }
        }

        async fn bring_up(&mut self, width: u32, height: u32, fps: u32) -> Result<(), EngineError> {
            self.pin_attempts = 0;
            let exe = sunshine::find_exe().ok_or(EngineError::NotInstalled("Sunshine"))?;
            tokio::fs::create_dir_all(&self.data_dir).await.map_err(EngineError::Io)?;
            cleanup_orphans(&self.data_dir).await;
            if sunshine::is_port_in_use().await {
                return Err(EngineError::AlreadyRunning("Sunshine"));
            }

            // Exactly the size the PC asked for; its ID is new every time.
            let display = self.display.insert(macdisplay::create_virtual(width, height, fps).await?).id;
            // Beside the MacBook, on the PC's side (the Mac sits on `mac_side` of the PC). A
            // display that stays put still works; only dragging across suffers, so this never fails.
            let toward_pc = crate::config::Config::load(&crate::config::Config::default_path())
                .map(|c| c.mac_side.opposite())
                .unwrap_or(macdisplay::Side::Left);
            if let Err(e) = macdisplay::place_virtual(display, toward_pc) {
                eprintln!("sidescreen: {e}");
            }

            let conf = sunshine::conf_path(&self.data_dir);
            tokio::fs::write(&conf, sunshine::conf_text(&display.to_string(), &self.data_dir))
                .await
                .map_err(EngineError::Io)?;
            sunshine::set_credentials(&exe, &conf, &self.user, &self.pass).await?;

            let host = self.host.insert(Host::spawn(&exe, &conf, &sunshine::log_path(&self.data_dir)).await?);
            host.wait_ready(SUNSHINE_WAIT).await
        }
    }

    #[async_trait]
    impl HostEngines for RealHostEngines {
        async fn start(&mut self, width: u32, height: u32, fps: u32) -> Result<(), EngineError> {
            let result = self.bring_up(width, height, fps).await;
            if result.is_err() {
                self.stop().await;
            }
            result
        }

        async fn submit_pin(&mut self, pin: &str, client_name: &str) -> Result<bool, EngineError> {
            let attempt = self.pin_attempts;
            self.pin_attempts += 1;
            match &self.host {
                Some(host) => host.submit_pin(&self.user, &self.pass, pin, client_name, attempt).await,
                None => Ok(false),
            }
        }

        async fn stop(&mut self) {
            if let Some(mut host) = self.host.take() {
                host.kill().await;
            }
            if let Some(display) = self.display.take() {
                display.remove().await;
            }
        }

        async fn exited(&mut self) -> &'static str {
            let sunshine = async {
                match self.host.as_mut() {
                    Some(host) => {
                        let _ = host.wait().await;
                    }
                    None => std::future::pending().await,
                }
            };
            let display = async {
                match self.display.as_mut() {
                    Some(display) => display.ended().await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                _ = sunshine => "Sunshine",
                _ = display => macdisplay::DISPLAY_PART,
            }
        }
    }
}
