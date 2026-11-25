//! The virtual display on the Mac: created by our helper (expand-display) at the requested
//! size, and placed beside the MacBook on the PC's side.

pub use crate::side::Side;

/// A screen rectangle in Mac points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}
#[cfg(target_os = "macos")]
use crate::sunshine::EngineError;
#[cfg(target_os = "macos")]
use std::time::Duration;

/// Where to put the virtual display's top-left corner: against the MacBook's edge facing the
/// PC, centred along it. That matches the desk when the viewer fills the PC monitor next to the
/// MacBook, and lets macOS carry a dragged window between the two screens.
pub fn beside_origin(main: Rect, (width, height): (i32, i32), toward_pc: Side) -> (i32, i32) {
    let centred_y = main.top + (main.bottom - main.top - height) / 2;
    let centred_x = main.left + (main.right - main.left - width) / 2;
    match toward_pc {
        Side::Left => (main.left - width, centred_y),
        Side::Right => (main.right, centred_y),
        Side::Top => (centred_x, main.top - height),
        Side::Bottom => (centred_x, main.bottom),
    }
}

/// Moves the virtual display beside the MacBook for this login session only. Never moves the
/// MacBook's own screen.
#[cfg(target_os = "macos")]
pub fn place_virtual(id: u32, toward_pc: Side) -> Result<(), String> {
    use core_graphics::display::{CGConfigureOption, CGDisplay};
    let main = CGDisplay::main();
    if main.id == id {
        return Err("the virtual display is the main display; left where it is".to_string());
    }
    let (m, v) = (main.bounds(), CGDisplay::new(id).bounds());
    let main_rect = Rect {
        left: m.origin.x as i32,
        top: m.origin.y as i32,
        right: (m.origin.x + m.size.width) as i32,
        bottom: (m.origin.y + m.size.height) as i32,
    };
    let (x, y) = beside_origin(main_rect, (v.size.width as i32, v.size.height as i32), toward_pc);
    let config = main.begin_configuration().map_err(|e| format!("display configuration failed ({e})"))?;
    if let Err(e) = CGDisplay::new(id).configure_display_origin(&config, x, y) {
        let _ = main.cancel_configuration(&config);
        return Err(format!("could not move the virtual display ({e})"));
    }
    main.complete_configuration(&config, CGConfigureOption::ConfigureForSession)
        .map_err(|e| format!("could not apply the display arrangement ({e})"))
}

/// The vendor number the helper gives Expand's display ("SS"), to tell it from real ones.
pub const VENDOR_ID: u32 = 0x5353;

/// Reads the helper's "ready <display id>" line.
pub fn parse_ready(line: &str) -> Option<u32> {
    line.trim().strip_prefix("ready ")?.trim().parse().ok()
}

/// Expand's virtual display: lives as long as the helper process that created it.
#[cfg(target_os = "macos")]
pub struct VirtualDisplay {
    helper: tokio::process::Child,
    /// The helper's stdin, held here: it quits when this closes, and tokio's `wait` would close
    /// it if it were still inside `helper`.
    _lifeline: Option<tokio::process::ChildStdin>,
    pub id: u32,
}

/// The helper ships inside the app, next to its executable.
#[cfg(target_os = "macos")]
fn helper_path() -> Option<std::path::PathBuf> {
    let path = std::env::current_exe().ok()?.parent()?.join("expand-display");
    path.exists().then_some(path)
}

/// Creates the virtual display at this size and refresh rate.
#[cfg(target_os = "macos")]
pub async fn create_virtual(width: u32, height: u32, fps: u32) -> Result<VirtualDisplay, EngineError> {
    use tokio::io::AsyncBufReadExt;
    let exe = helper_path().ok_or(EngineError::NotInstalled(DISPLAY_PART))?;
    let mut helper = tokio::process::Command::new(exe)
        .args([width.to_string(), height.to_string(), fps.to_string()])
        // Kept open for the display's lifetime: if this app dies, the helper sees it close.
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        // Only read when it fails to start; it says nothing once running.
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(EngineError::Io)?;
    let stdout = helper.stdout.take().expect("stdout is piped");
    let mut lines = tokio::io::BufReader::new(stdout).lines();
    let first = tokio::time::timeout(Duration::from_secs(10), lines.next_line()).await;
    match first {
        Ok(Ok(Some(line))) => match parse_ready(&line) {
            Some(id) => {
                // Placed and streamed only once macOS lists it.
                for _ in 0..40 {
                    if core_graphics::display::CGDisplay::active_displays().unwrap_or_default().contains(&id) {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                let lifeline = helper.stdin.take();
                Ok(VirtualDisplay { helper, _lifeline: lifeline, id })
            }
            None => Err(EngineError::StartFailed(DISPLAY_PART, line)),
        },
        Ok(_) => {
            let mut reason = String::new();
            if let Some(mut err) = helper.stderr.take() {
                use tokio::io::AsyncReadExt;
                let _ = err.read_to_string(&mut reason).await;
            }
            Err(EngineError::StartFailed(DISPLAY_PART, reason.trim().to_string()))
        }
        Err(_) => Err(EngineError::StartTimeout(DISPLAY_PART)),
    }
}

#[cfg(target_os = "macos")]
impl VirtualDisplay {
    /// Waits until the helper (and with it the display) is gone.
    pub async fn ended(&mut self) {
        let _ = self.helper.wait().await;
    }

    /// Removes the display.
    pub async fn remove(mut self) {
        let _ = self.helper.kill().await;
    }
}

/// How the virtual display is named in messages.
#[cfg(target_os = "macos")]
pub const DISPLAY_PART: &str = "Virtual display";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_helpers_ready_line_gives_the_display_id() {
        assert_eq!(parse_ready("ready 38"), Some(38));
        assert_eq!(parse_ready("ready 38
"), Some(38));
        assert_eq!(parse_ready("could not set the display"), None);
        assert_eq!(parse_ready("ready"), None);
    }




    // The MacBook's screen and a 2560x1440 virtual display, as measured on the Mac (points).
    const MAIN: Rect = Rect { left: 0, top: 0, right: 1728, bottom: 1117 };
    const SIZE: (i32, i32) = (2560, 1440);

    fn placed(toward_pc: Side) -> Rect {
        let (x, y) = beside_origin(MAIN, SIZE, toward_pc);
        Rect { left: x, top: y, right: x + SIZE.0, bottom: y + SIZE.1 }
    }

    // With the viewer filling the PC monitor next to the MacBook, the virtual display is the
    // screen between the PC and the MacBook, so a dragged window can cross from one to the
    // other (decided 2026-11-18, replacing the corner placement).
    #[test]
    fn with_the_pc_on_the_left_the_virtual_display_sits_against_the_macbooks_left_edge() {
        assert_eq!(beside_origin(MAIN, SIZE, Side::Left), (-2560, -161));
    }

    #[test]
    fn with_the_pc_on_the_right_the_virtual_display_sits_against_the_macbooks_right_edge() {
        assert_eq!(beside_origin(MAIN, SIZE, Side::Right), (1728, -161));
    }

    #[test]
    fn the_virtual_display_shares_the_edge_facing_the_pc_without_overlapping() {
        for toward_pc in [Side::Left, Side::Right, Side::Top, Side::Bottom] {
            let v = placed(toward_pc);
            let touching = match toward_pc {
                Side::Left => v.right == MAIN.left,
                Side::Right => v.left == MAIN.right,
                Side::Top => v.bottom == MAIN.top,
                Side::Bottom => v.top == MAIN.bottom,
            };
            let overlapping =
                v.left < MAIN.right && v.right > MAIN.left && v.top < MAIN.bottom && v.bottom > MAIN.top;
            assert!(touching && !overlapping, "toward {toward_pc:?}: {v:?}");
        }
    }

}
