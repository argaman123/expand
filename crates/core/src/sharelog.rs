//! A short log of what sharing does (connections, hand-overs, clipboard, language), kept in
//! the app's settings folder as `sharing.log`. Small: it starts over when it grows past 256 KB.

use std::io::Write;

pub fn path() -> std::path::PathBuf {
    crate::config::Config::default_path().with_file_name("sharing.log")
}

pub fn line(text: impl AsRef<str>) {
    let path = path();
    if std::fs::metadata(&path).map(|m| m.len() > 256 * 1024).unwrap_or(false) {
        let _ = std::fs::remove_file(&path);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default();
        let _ = writeln!(file, "{now} {}", text.as_ref());
    }
}
