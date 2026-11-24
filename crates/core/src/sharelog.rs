//! A short log of what sharing does (connections, hand-overs, clipboard, language), kept in
//! the app's settings folder as `sharing.log`. Small: it starts over when it grows past 256 KB.
//! Written only once the app has called [`enable`], so tests never touch the owner's log.

use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

static ENABLED: AtomicBool = AtomicBool::new(false);

/// Starts writing the log. The app calls this when it starts; nothing else does.
pub fn enable() {
    ENABLED.store(true, Ordering::Relaxed);
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

pub fn path() -> std::path::PathBuf {
    crate::config::Config::default_path().with_file_name("sharing.log")
}

pub fn line(text: impl AsRef<str>) {
    if enabled() {
        write_line(&path(), text.as_ref());
    }
}

fn write_line(path: &Path, text: &str) {
    if std::fs::metadata(path).map(|m| m.len() > 256 * 1024).unwrap_or(false) {
        let _ = std::fs::remove_file(path);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default();
        let _ = writeln!(file, "{now} {text}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Live check, task 9: `cargo test` wrote its lines into the owner's real sharing.log.
    #[test]
    fn nothing_is_logged_until_the_app_enables_it() {
        assert!(!enabled());
    }

    #[test]
    fn lines_are_appended_with_a_time() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sharing.log");
        write_line(&path, "one");
        write_line(&path, "two");
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].ends_with(" one") && lines[1].ends_with(" two"), "{text}");
    }

    #[test]
    fn a_log_past_its_size_starts_over() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sharing.log");
        std::fs::write(&path, vec![b'x'; 256 * 1024 + 1]).unwrap();
        write_line(&path, "fresh");
        assert!(std::fs::read_to_string(&path).unwrap().ends_with(" fresh\n"));
    }
}
