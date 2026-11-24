//! A dev-only trace of the viewer's windows and of the app's own display and bar windows: each
//! start of Moonlight, each of its windows made, shown, moved out of sight or embedded, and how
//! long any of them was on a monitor, with millisecond times. For proving in one live run that
//! Moonlight never shows, and for finding a flash.
//!
//! Off unless the app runs with `SIDESCREEN_TRACE_WINDOWS=1`; then the lines go to
//! `windows-trace.log` beside `sharing.log` (and to stderr). Off, a call costs one read of a flag
//! set once (a line is built only when it is on). Never written by tests (the app must
//! have enabled the shared log, as for `sharing.log`).

use std::io::Write;
use std::path::Path;

/// The environment variable that turns the trace on (`1`).
pub const ENV: &str = "SIDESCREEN_TRACE_WINDOWS";

/// Whether a value of [`ENV`] turns the trace on.
fn wanted(value: Option<&std::ffi::OsStr>) -> bool {
    value.is_some_and(|v| v == "1")
}

/// Whether the trace is on: asked for, in the running app. Read once.
pub fn on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| wanted(std::env::var_os(ENV).as_deref())) && crate::sharelog::enabled()
}

pub fn path() -> std::path::PathBuf {
    crate::config::Config::default_path().with_file_name("windows-trace.log")
}

/// Milliseconds since the Unix epoch: lines from the app's threads and Windows' own event times
/// line up through it.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

/// One line: the time in ms, then what happened.
fn format_line(ms: u64, text: &str) -> String {
    format!("{}.{:03} {text}", ms / 1000, ms % 1000)
}

/// Writes the line `make` builds, when the trace is on (off, it is never built).
pub fn line(make: impl FnOnce() -> String) {
    if on() {
        let line = format_line(now_ms(), &make());
        eprintln!("sidescreen trace: {line}");
        write_line(&path(), &line);
    }
}

fn write_line(path: &Path, line: &str) {
    if std::fs::metadata(path).map(|m| m.len() > 1024 * 1024).unwrap_or(false) {
        let _ = std::fs::remove_file(path);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{line}");
    }
}

/// How long each of the viewer's windows was on a monitor, from what is seen of it (event
/// times, in ms): it starts counting when a window is seen shown on a monitor, and gives the
/// time once it is seen out of sight, hidden, embedded or gone.
#[derive(Debug, Default)]
pub struct Exposure {
    since: std::collections::HashMap<isize, u64>,
}

impl Exposure {
    /// `window` was seen at `at` (ms), on a monitor and shown (`showing`) or not. Returns how
    /// long it showed when this ends a stretch of showing.
    pub fn seen(&mut self, window: isize, at: u64, showing: bool) -> Option<u64> {
        if showing {
            self.since.entry(window).or_insert(at);
            None
        } else {
            self.since.remove(&window).map(|from| at.saturating_sub(from))
        }
    }

    /// The windows still showing, and since when.
    pub fn showing(&self) -> Vec<(isize, u64)> {
        let mut all: Vec<_> = self.since.iter().map(|(w, t)| (*w, *t)).collect();
        all.sort();
        all
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_1_turns_it_on() {
        assert!(wanted(Some("1".as_ref())));
        assert!(!wanted(Some("0".as_ref())));
        assert!(!wanted(Some("".as_ref())));
        assert!(!wanted(None));
    }

    #[test]
    fn tests_never_write_it() {
        assert!(!on());
    }

    #[test]
    fn a_line_starts_with_seconds_and_milliseconds() {
        assert_eq!(format_line(1_764_000_000_042, "show"), "1764000000.042 show");
    }

    #[test]
    fn lines_are_appended() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("windows-trace.log");
        write_line(&path, "one");
        write_line(&path, "two");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one\ntwo\n");
    }

    #[test]
    fn a_window_shown_then_put_away_showed_for_the_time_between() {
        let mut e = Exposure::default();
        assert_eq!(e.seen(7, 1000, true), None);
        // Seen again while it shows: still counted from the first time.
        assert_eq!(e.seen(7, 1010, true), None);
        assert_eq!(e.seen(7, 1016, false), Some(16));
        assert!(e.showing().is_empty());
    }

    #[test]
    fn a_window_never_seen_showing_showed_for_no_time() {
        let mut e = Exposure::default();
        assert_eq!(e.seen(7, 1000, false), None);
    }

    #[test]
    fn windows_are_counted_each_on_its_own() {
        let mut e = Exposure::default();
        e.seen(1, 100, true);
        e.seen(2, 150, true);
        assert_eq!(e.seen(1, 160, false), Some(60));
        assert_eq!(e.showing(), vec![(2, 150)]);
    }
}
