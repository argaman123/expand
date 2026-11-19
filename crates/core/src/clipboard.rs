//! Clipboard sharing: what is copied on one computer can be pasted on the other. Text, images
//! and files. This file holds the parts that need no clipboard: noticing changes without
//! echoing back what the other side sent, and packing contents into messages (files in small
//! pieces, so mouse and keyboard keep flowing while a large file crosses).

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Files larger than this in total are not sent (copying a whole disk by accident).
pub const MAX_FILES_BYTES: u64 = 512 * 1024 * 1024;
/// Size of one piece of a file on the wire.
pub const CHUNK: usize = 256 * 1024;

/// What a clipboard holds, as far as sharing goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Content {
    Text(String),
    /// RGBA pixels.
    Image { width: usize, height: usize, rgba: Vec<u8> },
    Files(Vec<PathBuf>),
}

impl Content {
    /// A fingerprint, to tell whether the clipboard changed.
    pub fn fingerprint(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        match self {
            Content::Text(t) => (0u8, t).hash(&mut h),
            Content::Image { width, height, rgba } => (1u8, width, height, rgba).hash(&mut h),
            Content::Files(paths) => (2u8, paths).hash(&mut h),
        }
        h.finish()
    }
}

/// One clipboard message between the computers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClipMsg {
    Text(String),
    /// A PNG, base64.
    Image(String),
    /// Files are coming: their names and sizes, in order. `id` ties the pieces together.
    FilesStart { id: u64, names: Vec<String>, sizes: Vec<u64> },
    /// One piece of file `file` (base64). Pieces arrive in order.
    FilePiece { id: u64, file: usize, data: String },
}

/// Packs a clipboard's contents into messages. `None` when there is nothing to send (too
/// large, unreadable).
pub fn pack(content: &Content, id: u64) -> Option<Vec<ClipMsg>> {
    match content {
        Content::Text(text) => Some(vec![ClipMsg::Text(text.clone())]),
        Content::Image { width, height, rgba } => {
            let mut png_bytes = Vec::new();
            {
                let mut encoder = png::Encoder::new(&mut png_bytes, *width as u32, *height as u32);
                encoder.set_color(png::ColorType::Rgba);
                encoder.set_depth(png::BitDepth::Eight);
                let mut writer = encoder.write_header().ok()?;
                writer.write_image_data(rgba).ok()?;
            }
            Some(vec![ClipMsg::Image(b64(&png_bytes))])
        }
        Content::Files(paths) => {
            let mut sizes = Vec::new();
            for path in paths {
                let meta = std::fs::metadata(path).ok()?;
                if !meta.is_file() {
                    return None;
                }
                sizes.push(meta.len());
            }
            if sizes.iter().sum::<u64>() > MAX_FILES_BYTES {
                return None;
            }
            let names = paths.iter().map(|p| file_name(p)).collect();
            let mut messages = vec![ClipMsg::FilesStart { id, names, sizes }];
            for (file, path) in paths.iter().enumerate() {
                let bytes = std::fs::read(path).ok()?;
                for piece in bytes.chunks(CHUNK) {
                    messages.push(ClipMsg::FilePiece { id, file, data: b64(piece) });
                }
            }
            Some(messages)
        }
    }
}

/// Rebuilds contents from messages. Files are written into `folder`.
pub struct Unpacker {
    folder: PathBuf,
    files: Option<FilesInProgress>,
}

struct FilesInProgress {
    id: u64,
    paths: Vec<PathBuf>,
    sizes: Vec<u64>,
    written: Vec<u64>,
}

impl Unpacker {
    pub fn new(folder: PathBuf) -> Unpacker {
        Unpacker { folder, files: None }
    }

    /// Feeds one message; returns the contents once they are complete.
    pub fn feed(&mut self, message: ClipMsg) -> Option<Content> {
        match message {
            ClipMsg::Text(text) => Some(Content::Text(text)),
            ClipMsg::Image(data) => {
                let bytes = unb64(&data)?;
                let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
                let mut reader = decoder.read_info().ok()?;
                let mut rgba = vec![0; reader.output_buffer_size()];
                let info = reader.next_frame(&mut rgba).ok()?;
                rgba.truncate(info.buffer_size());
                if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
                    return None;
                }
                Some(Content::Image { width: info.width as usize, height: info.height as usize, rgba })
            }
            ClipMsg::FilesStart { id, names, sizes } => {
                let dir = self.folder.join(id.to_string());
                std::fs::create_dir_all(&dir).ok()?;
                let mut paths = Vec::new();
                for name in &names {
                    // Only the name, never a path from the other side.
                    let safe = Path::new(name).file_name().map(|n| n.to_owned()).unwrap_or_else(|| "file".into());
                    let path = dir.join(safe);
                    std::fs::write(&path, b"").ok()?;
                    paths.push(path);
                }
                let written = vec![0; sizes.len()];
                self.files = Some(FilesInProgress { id, paths, sizes, written });
                self.finish_files()
            }
            ClipMsg::FilePiece { id, file, data } => {
                let progress = self.files.as_mut().filter(|f| f.id == id)?;
                let bytes = unb64(&data)?;
                let path = progress.paths.get(file)?;
                use std::io::Write;
                std::fs::OpenOptions::new().append(true).open(path).ok()?.write_all(&bytes).ok()?;
                progress.written[file] += bytes.len() as u64;
                self.finish_files()
            }
        }
    }

    fn finish_files(&mut self) -> Option<Content> {
        let done = self.files.as_ref().is_some_and(|f| f.written == f.sizes);
        if done {
            self.files.take().map(|f| Content::Files(f.paths))
        } else {
            None
        }
    }
}

/// Notices clipboard changes made here, ignoring the ones that came from the other computer.
#[derive(Default)]
pub struct ChangeWatch {
    last: Option<u64>,
    /// The first look only records what is there.
    started: bool,
}

impl ChangeWatch {
    /// Given what the clipboard holds now, returns it if it is a new local change.
    pub fn local_change(&mut self, now: Option<&Content>) -> bool {
        let first = !std::mem::replace(&mut self.started, true);
        let Some(now) = now else { return false };
        let fingerprint = Some(now.fingerprint());
        if first {
            self.last = fingerprint;
            return false;
        }
        if fingerprint == self.last {
            return false;
        }
        self.last = fingerprint;
        true
    }

    /// Records contents just put on the clipboard from the other computer, so they are not
    /// sent back.
    pub fn applied(&mut self, content: &Content) {
        self.last = Some(content.fingerprint());
    }
}

/// The system clipboard, through arboard.
pub struct Local {
    inner: arboard::Clipboard,
}

impl Local {
    pub fn open() -> Option<Local> {
        arboard::Clipboard::new().ok().map(|inner| Local { inner })
    }

    /// A number that changes whenever anything is copied: cheap to ask, so the clipboard is
    /// only read when something new is on it.
    pub fn change_count(&self) -> u64 {
        #[cfg(windows)]
        unsafe {
            windows::Win32::System::DataExchange::GetClipboardSequenceNumber() as u64
        }
        #[cfg(target_os = "macos")]
        unsafe {
            use std::ffi::{c_char, c_void};
            #[link(name = "objc")]
            extern "C" {
                fn objc_getClass(name: *const c_char) -> *mut c_void;
                fn sel_registerName(name: *const c_char) -> *mut c_void;
                fn objc_msgSend();
            }
            #[link(name = "AppKit", kind = "framework")]
            extern "C" {}
            let send: unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void =
                std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
            let count: unsafe extern "C" fn(*mut c_void, *mut c_void) -> isize =
                std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
            let class = objc_getClass(c"NSPasteboard".as_ptr());
            let board = send(class, sel_registerName(c"generalPasteboard".as_ptr()));
            count(board, sel_registerName(c"changeCount".as_ptr())) as u64
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        0
    }

    /// What is on the clipboard now: files first, then text, then an image.
    pub fn read(&mut self) -> Option<Content> {
        if let Ok(files) = self.inner.get().file_list() {
            if !files.is_empty() {
                return Some(Content::Files(files));
            }
        }
        if let Ok(text) = self.inner.get_text() {
            if !text.is_empty() {
                return Some(Content::Text(text));
            }
        }
        let image = self.inner.get_image().ok()?;
        Some(Content::Image { width: image.width, height: image.height, rgba: image.bytes.into_owned() })
    }

    pub fn write(&mut self, content: &Content) -> bool {
        match content {
            Content::Text(text) => self.inner.set_text(text.clone()).is_ok(),
            Content::Image { width, height, rgba } => self
                .inner
                .set_image(arboard::ImageData { width: *width, height: *height, bytes: rgba.as_slice().into() })
                .is_ok(),
            Content::Files(paths) => self.inner.set().file_list(paths).is_ok(),
        }
    }
}

/// Where files copied on the other computer land, so they can be pasted here. Emptied when
/// sharing starts.
pub fn landing_folder() -> PathBuf {
    let folder = dirs::cache_dir().unwrap_or_else(std::env::temp_dir).join("sidescreen").join("clipboard");
    let _ = std::fs::remove_dir_all(&folder);
    let _ = std::fs::create_dir_all(&folder);
    folder
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn unb64(text: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::STANDARD.decode(text).ok()
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(content: &Content, folder: &Path) -> Option<Content> {
        let mut unpacker = Unpacker::new(folder.to_path_buf());
        let mut result = None;
        for message in pack(content, 7)? {
            // Every message survives the wire as JSON.
            let message: ClipMsg = serde_json::from_str(&serde_json::to_string(&message).unwrap()).unwrap();
            if let Some(done) = unpacker.feed(message) {
                result = Some(done);
            }
        }
        result
    }

    #[test]
    fn text_crosses_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let text = Content::Text("שלום, hello 👋".into());
        assert_eq!(roundtrip(&text, dir.path()), Some(text));
    }

    #[test]
    fn an_image_crosses_pixel_for_pixel() {
        let dir = tempfile::tempdir().unwrap();
        let rgba: Vec<u8> = (0..4 * 3 * 2).map(|i| (i * 11) as u8).collect();
        let image = Content::Image { width: 3, height: 2, rgba };
        assert_eq!(roundtrip(&image, dir.path()), Some(image));
    }

    #[test]
    fn files_cross_in_pieces_and_land_in_the_folder() {
        let from = tempfile::tempdir().unwrap();
        let to = tempfile::tempdir().unwrap();
        let big: Vec<u8> = (0..(CHUNK * 2 + 123)).map(|i| (i % 251) as u8).collect();
        std::fs::write(from.path().join("big.bin"), &big).unwrap();
        std::fs::write(from.path().join("note.txt"), "hi").unwrap();
        let files = Content::Files(vec![from.path().join("big.bin"), from.path().join("note.txt")]);

        let messages = pack(&files, 1).unwrap();
        assert!(messages.len() >= 4, "the big file goes in several pieces: {}", messages.len());

        let Some(Content::Files(landed)) = roundtrip(&files, to.path()) else { panic!("no files") };
        assert_eq!(landed.len(), 2);
        assert!(landed.iter().all(|p| p.starts_with(to.path())));
        assert_eq!(std::fs::read(&landed[0]).unwrap(), big);
        assert_eq!(std::fs::read_to_string(&landed[1]).unwrap(), "hi");
        assert_eq!(landed[1].file_name().unwrap(), "note.txt");
    }

    #[test]
    fn an_empty_file_still_arrives() {
        let from = tempfile::tempdir().unwrap();
        let to = tempfile::tempdir().unwrap();
        std::fs::write(from.path().join("empty"), "").unwrap();
        let files = Content::Files(vec![from.path().join("empty")]);
        let Some(Content::Files(landed)) = roundtrip(&files, to.path()) else { panic!("no files") };
        assert_eq!(std::fs::read(&landed[0]).unwrap(), b"");
    }

    #[test]
    fn folders_and_missing_files_are_not_sent() {
        let from = tempfile::tempdir().unwrap();
        assert!(pack(&Content::Files(vec![from.path().to_path_buf()]), 1).is_none());
        assert!(pack(&Content::Files(vec![from.path().join("gone")]), 1).is_none());
    }

    // Starting up must not push whatever happened to be on the clipboard across.
    #[test]
    fn what_was_on_the_clipboard_at_the_start_is_not_sent() {
        let mut watch = ChangeWatch::default();
        assert!(!watch.local_change(Some(&Content::Text("old".into()))));
        assert!(watch.local_change(Some(&Content::Text("new".into()))));
    }

    #[test]
    fn a_new_local_copy_is_noticed_once() {
        let mut watch = ChangeWatch::default();
        watch.local_change(None);
        let a = Content::Text("a".into());
        assert!(watch.local_change(Some(&a)));
        assert!(!watch.local_change(Some(&a)), "same contents again is not a change");
        assert!(!watch.local_change(None), "an empty clipboard is not something to send");
        assert!(watch.local_change(Some(&Content::Text("b".into()))));
    }

    // What arrived from the other computer must not bounce straight back.
    #[test]
    fn what_came_from_the_other_side_is_not_sent_back() {
        let mut watch = ChangeWatch::default();
        let remote = Content::Text("from the Mac".into());
        watch.applied(&remote);
        assert!(!watch.local_change(Some(&remote)));
    }
}
