//! The real engines behind the session traits.

use super::ViewerEngine;
use crate::config::{Placement, StreamSettings};
use crate::moonlight::{self, Viewer};
use async_trait::async_trait;
use std::path::PathBuf;
#[cfg(windows)]
use std::sync::Arc;

/// How long to look for the viewer's window before giving up on embedding it.
pub const EMBED_WAIT_MS: u64 = 15_000;

/// What to do about the viewer's window, while looking for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbedPlan {
    /// Put this window in the display window.
    Embed(isize),
    /// Not there yet: look again.
    Wait,
    /// Still none: bring the viewer's own window (its launcher, saying how it is getting on or
    /// why it can't) to the user, over the display window, and keep looking.
    ShowViewer,
    /// Leave the viewer in its own window.
    Fallback,
}

/// Where the look for the viewer's stream window is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Looking {
    pub waited_ms: u64,
    /// Its own window was brought to the user already ([`EmbedPlan::ShowViewer`]).
    pub viewer_shown: bool,
    /// A window of the viewer other than the ones it speaks in shows on a monitor in its own
    /// frame: its stream, in a window not known as the stream's.
    pub own_window_shows: bool,
}

/// The window found so far (if any) and where the look is decide the next step. After
/// [`EMBED_WAIT_MS`] with none: the viewer keeps a stream window of its own that shows; else its
/// own window is brought to the user once, and the look goes on (a slow stream still goes in; an
/// error the viewer puts up ends the session, [`ViewerEngine::last_words`]).
pub fn embed_plan(found: Option<isize>, now: Looking) -> EmbedPlan {
    match found {
        Some(window) => EmbedPlan::Embed(window),
        None if now.waited_ms < EMBED_WAIT_MS || now.viewer_shown => EmbedPlan::Wait,
        None if now.own_window_shows => EmbedPlan::Fallback,
        None => EmbedPlan::ShowViewer,
    }
}

/// Where the app puts the viewer's window, for one session. The display window is up from the
/// session's start, with "Starting" in it until a viewer window is in.
#[cfg(windows)]
pub trait ViewerHost: Send + Sync {
    /// Embeds the window in the display window. An error leaves the window as it was, in its own
    /// frame.
    fn attach(&self, window: isize) -> Result<(), String>;
    /// The embedded window is gone: "Starting" shows until the next one is attached.
    fn detach(&self);
    /// No window could be embedded: the viewer keeps its own, and the display window goes.
    fn fallback(&self);
    /// The viewer is stopping: refuse any attach still under way. What the display window does
    /// is the session end's business.
    fn end(&self);
    /// Where the display window is on screen while it shows (a window the viewer speaks in is
    /// brought over it).
    fn area(&self) -> Option<crate::share::geometry::Area> {
        None
    }
}

/// The error the viewer put up for the user, once, and whether it was read from its log (before
/// its stream window was in: a later stream window means the start went on after all).
#[cfg(windows)]
#[derive(Default)]
struct Said {
    text: std::sync::Mutex<Option<(String, bool)>>,
    told: tokio::sync::Notify,
}

#[cfg(windows)]
impl Said {
    fn say(&self, text: String, from_log: bool) {
        let mut said = self.text.lock().unwrap();
        if said.is_none() {
            *said = Some((text, from_log));
            self.told.notify_one();
        }
    }
}

/// How long a window the viewer speaks in must still show before it is answered (one shown for a
/// moment as the viewer quits is not).
#[cfg(windows)]
const SPEAK_GRACE: std::time::Duration = std::time::Duration::from_millis(400);
/// How long an error read from the viewer's log waits for a stream window that would say the start
/// went on after all.
#[cfg(windows)]
const LOG_ERROR_GRACE: std::time::Duration = std::time::Duration::from_secs(1);

/// A run of the viewer before its stream (`list`, `pair`) while the display window shows
/// Starting: its windows can take the front as the stream's do, so its process is recorded as
/// the viewer's ([`set_viewer_process`](crate::winwindow::set_viewer_process)) from its start
/// until this is dropped (it ended, or it was given up and killed). Dropped late (after the
/// stream started), it leaves the stream's process recorded. Its windows are kept out of sight
/// meanwhile, as the stream's are ([`Concealer`](crate::winwindow::Concealer)).
#[cfg_attr(not(windows), allow(dead_code))]
struct HelperShown {
    embedding: bool,
    pid: std::sync::atomic::AtomicU32,
    #[cfg(windows)]
    concealer: std::sync::Mutex<Option<crate::winwindow::Concealer>>,
}

impl HelperShown {
    fn started(&self, _pid: u32) {
        #[cfg(windows)]
        if self.embedding {
            self.pid.store(_pid, std::sync::atomic::Ordering::Relaxed);
            crate::winwindow::set_viewer_process(Some(_pid));
            *self.concealer.lock().unwrap() = Some(crate::winwindow::Concealer::start(_pid));
        }
    }
}

impl Drop for HelperShown {
    fn drop(&mut self) {
        #[cfg(windows)]
        match *self.pid.get_mut() {
            0 => {}
            pid => crate::winwindow::forget_viewer_process(pid),
        }
    }
}

/// Moonlight on the PC.
pub struct RealViewer {
    exe: PathBuf,
    viewer: Option<Viewer>,
    /// Where the Mac sits, and the monitor chosen for the stream: the viewer opens on that
    /// monitor, or else the one on the Mac's side.
    #[cfg_attr(not(windows), allow(dead_code))]
    placement: Placement,
    #[cfg_attr(not(windows), allow(dead_code))]
    stream_monitor: Option<u32>,
    /// The display window the viewer's window goes into; without one it keeps its own frame.
    #[cfg(windows)]
    host: Option<Arc<dyn ViewerHost>>,
    /// Looks after the viewer's window while it runs.
    #[cfg(windows)]
    window_task: Option<tokio::task::JoinHandle<()>>,
    /// The stream viewer's concealer, while it runs (shared with the task that embeds its window,
    /// which gives its windows back on the fallback): dropped before the viewer is killed, so its
    /// in-context hook is gone before the viewer's process number can be anyone else's.
    #[cfg(windows)]
    stream_concealer: Arc<std::sync::Mutex<Option<crate::winwindow::Concealer>>>,
    /// Answers the viewer when it speaks to the user ([`answer_viewer`]).
    #[cfg(windows)]
    speak_task: Option<tokio::task::JoinHandle<()>>,
    /// The error the viewer put up, for this start.
    #[cfg(windows)]
    said: Arc<Said>,
    /// Whether its stream window is in the display window now.
    #[cfg(windows)]
    streaming: Arc<std::sync::atomic::AtomicBool>,
}

impl RealViewer {
    pub fn new(placement: Placement, stream_monitor: Option<u32>) -> std::io::Result<RealViewer> {
        let exe = moonlight::find_exe().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "Moonlight is not installed on this PC")
        })?;
        Ok(RealViewer {
            exe,
            viewer: None,
            placement,
            stream_monitor,
            #[cfg(windows)]
            host: None,
            #[cfg(windows)]
            window_task: None,
            #[cfg(windows)]
            stream_concealer: Default::default(),
            #[cfg(windows)]
            speak_task: None,
            #[cfg(windows)]
            said: Default::default(),
            #[cfg(windows)]
            streaming: Default::default(),
        })
    }

    /// Runs this Moonlight instead of the one installed (dev probes, with a portable copy).
    pub fn with_exe(mut self, exe: PathBuf) -> RealViewer {
        self.exe = exe;
        self
    }

    /// Embeds the viewer's window in `host` (see [`ViewerHost`]).
    #[cfg(windows)]
    pub fn with_host(mut self, host: Arc<dyn ViewerHost>) -> RealViewer {
        self.host = Some(host);
        self
    }

    /// For a run of the viewer before its stream (asking whether it is paired, pairing): while
    /// it runs, its windows count as the display's, as the stream's do ([`HelperShown`]).
    fn helper_shown(&self) -> HelperShown {
        #[cfg(windows)]
        let embedding = self.host.is_some() && !embedding_disabled();
        #[cfg(not(windows))]
        let embedding = false;
        HelperShown {
            embedding,
            pid: Default::default(),
            #[cfg(windows)]
            concealer: Default::default(),
        }
    }

    #[cfg(windows)]
    fn forget_window(&mut self) {
        if let Some(task) = self.window_task.take() {
            task.abort();
        }
        if let Some(task) = self.speak_task.take() {
            task.abort();
        }
        // Unhooked now (the concealer's thread ends before this returns), not whenever the
        // aborted task is dropped.
        drop(self.stream_concealer.lock().unwrap().take());
        crate::winwindow::set_embedded_viewer(None);
        crate::winwindow::set_viewer_process(None);
    }
}

#[async_trait]
impl ViewerEngine for RealViewer {
    async fn is_paired(&mut self, host: &str) -> std::io::Result<Option<bool>> {
        let shown = self.helper_shown();
        moonlight::is_paired(&self.exe, host, |pid| shown.started(pid)).await
    }

    async fn pair(&mut self, host: &str, pin: &str) -> std::io::Result<bool> {
        let shown = self.helper_shown();
        moonlight::pair(&self.exe, host, pin, |pid| shown.started(pid)).await
    }

    async fn start(&mut self, host: &str, s: &StreamSettings) -> std::io::Result<()> {
        #[cfg(windows)]
        let embedding = self.host.is_some() && !embedding_disabled();
        #[cfg(not(windows))]
        let embedding = false;
        let hidden = moonlight::stream_starts_hidden(embedding, std::env::var_os("EXPAND_VIEWER_PLAIN").as_deref());
        #[cfg_attr(not(windows), allow(unused_mut))]
        let mut viewer = Viewer::spawn(&self.exe, host, s, hidden)?;
        #[cfg(windows)]
        if let Some(pid) = viewer.pid() {
            self.forget_window();
            self.said = Default::default();
            self.streaming = Default::default();
            if let Some(log) = viewer.take_log() {
                read_viewer_log(pid, log, Arc::clone(&self.said), Arc::clone(&self.streaming));
            }
            let full_screen = s.window_mode != crate::config::WindowMode::Windowed;
            let (placement, monitor) = (self.placement, self.stream_monitor);
            self.window_task = Some(match self.host.clone().filter(|_| embedding) {
                Some(host) => {
                    // Its windows count as the display's while it starts (they take the front).
                    crate::winwindow::set_viewer_process(Some(pid));
                    // Its stream window out of sight from the moment it appears, until it is in
                    // the display window, and in the moment it shows; a window it speaks to the
                    // user in is answered at once.
                    let found = Arc::new(tokio::sync::Notify::new());
                    let (speaks, heard) = tokio::sync::mpsc::unbounded_channel();
                    let tell = Arc::clone(&found);
                    let (hook_env, plain_env) = (std::env::var_os("EXPAND_VIEWER_HOOK"), std::env::var_os("EXPAND_VIEWER_PLAIN"));
                    let machine = moonlight::machine_of(&self.exe);
                    let in_context = moonlight::viewer_hook_wanted(hook_env.as_deref(), plain_env.as_deref(), machine);
                    crate::trace::line(|| {
                        format!(
                            "viewer {pid} in-context hook {}: EXPAND_VIEWER_HOOK {hook_env:?}, EXPAND_VIEWER_PLAIN {plain_env:?}, viewer {} is {machine:x?}, this app {:x?}",
                            if in_context { "wanted" } else { "not wanted" },
                            self.exe.display(),
                            moonlight::own_machine(),
                        )
                    });
                    let concealer = crate::winwindow::Concealer::start_telling(pid, in_context, move |h| match h {
                        crate::winwindow::Heard::StreamShown(_) => tell.notify_one(),
                        crate::winwindow::Heard::Speaks(window) => {
                            let _ = speaks.send(window);
                        }
                    });
                    self.speak_task = Some(tokio::spawn(answer_viewer(heard, Arc::clone(&host), Arc::clone(&self.said))));
                    let streaming = Arc::clone(&self.streaming);
                    *self.stream_concealer.lock().unwrap() = Some(concealer);
                    let concealer = Arc::clone(&self.stream_concealer);
                    tokio::spawn(embed_viewer_window(pid, host, concealer, found, streaming, placement, monitor, full_screen))
                }
                None => tokio::spawn(settle_viewer_window(pid, placement, monitor, full_screen)),
            });
        }
        self.viewer = Some(viewer);
        Ok(())
    }

    async fn closed(&mut self) {
        #[cfg(windows)]
        let (said, streaming) = (Arc::clone(&self.said), Arc::clone(&self.streaming));
        match self.viewer.as_mut() {
            Some(v) => {
                let _pid = v.pid();
                #[cfg(windows)]
                loop {
                    tokio::select! {
                        _ = v.wait() => break,
                        _ = said.told.notified() => {
                            let from_log = said.text.lock().unwrap().as_ref().is_some_and(|(_, log)| *log);
                            if from_log {
                                // A stream window coming in after all says the start went on.
                                tokio::time::sleep(LOG_ERROR_GRACE).await;
                                if streaming.load(std::sync::atomic::Ordering::Relaxed) {
                                    *said.text.lock().unwrap() = None;
                                    continue;
                                }
                            }
                            return;
                        }
                    }
                }
                #[cfg(not(windows))]
                let _ = v.wait().await;
                // It ended by itself: its process number may be another's from now on.
                #[cfg(windows)]
                if let Some(pid) = _pid {
                    crate::winwindow::forget_viewer_process(pid);
                }
            }
            None => std::future::pending().await,
        }
    }

    fn last_words(&mut self) -> Option<String> {
        #[cfg(windows)]
        return self.said.text.lock().unwrap().take().map(|(text, _)| text);
        #[cfg(not(windows))]
        None
    }

    async fn stop(&mut self) {
        #[cfg(windows)]
        {
            self.forget_window();
            // No viewer window goes into the display window once this is said; the one in it
            // leaves "Starting" behind.
            if let Some(host) = &self.host {
                host.end();
            }
        }
        if let Some(mut v) = self.viewer.take() {
            v.kill().await;
        }
    }
}

/// `EXPAND_NO_EMBED=1` (dev only, read once) keeps the viewer in its own window, on the
/// chosen monitor, and never shows the display window: for trying the fallback.
#[cfg(windows)]
pub fn embedding_disabled() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OFF.get_or_init(|| std::env::var_os("EXPAND_NO_EMBED").is_some_and(|v| v == "1"))
}

#[cfg(windows)]
const EMBED_POLL: std::time::Duration = std::time::Duration::from_millis(250);

/// Reads the viewer's log as it writes it (on a thread of its own, to its end: the viewer stalls
/// if it is not read), and hears an error it puts up for the user ([`viewer_error`]) while its
/// stream window is not in yet: the start failed, with that to say.
///
/// [`viewer_error`]: crate::layout::viewer_error
#[cfg(windows)]
fn read_viewer_log(pid: u32, log: std::fs::File, said: Arc<Said>, streaming: Arc<std::sync::atomic::AtomicBool>) {
    use std::io::BufRead;
    let _ = std::thread::Builder::new().name("expand-viewer-log".into()).spawn(move || {
        let mut log = std::io::BufReader::new(log);
        let mut line = Vec::new();
        loop {
            line.clear();
            match log.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let text = String::from_utf8_lossy(&line);
            if let Some(error) = crate::layout::viewer_error(&text) {
                let in_stream = streaming.load(std::sync::atomic::Ordering::Relaxed);
                // Not its text: it can name the host.
                crate::trace::line(|| format!("viewer {pid} put up an error ({} chars){}", error.len(), if in_stream { ", streaming" } else { "" }));
                if !in_stream {
                    said.say(error.to_string(), true);
                }
            }
        }
    });
}

/// Answers the viewer each time it speaks to the user in a window of its own (`heard`: the
/// concealer tells, [`Heard::Speaks`](crate::winwindow::Heard::Speaks)): when the window still
/// shows a moment later, a message box's text goes to the session ([`Said`]: it fails with it,
/// shown in the display window), and any other window (its launcher with an error or a prompt,
/// a message box that can't be read) is brought to the user, over the display window, in front.
#[cfg(windows)]
async fn answer_viewer(mut heard: tokio::sync::mpsc::UnboundedReceiver<isize>, host: Arc<dyn ViewerHost>, said: Arc<Said>) {
    use crate::winwindow;
    while let Some(window) = heard.recv().await {
        tokio::time::sleep(SPEAK_GRACE).await;
        if !winwindow::is_window(window) || !winwindow::is_visible(window) {
            continue;
        }
        if winwindow::is_message_box(window) {
            let text = tokio::task::spawn_blocking(move || winwindow::message_text(window)).await.ok().flatten();
            if let Some(text) = text {
                said.say(text, false);
                continue;
            }
        }
        let over = host.area();
        winwindow::bring_to_user(window, over);
    }
}

/// Puts the viewer's window in the display window as soon as it appears (the `concealer` tells
/// `found` the moment it shows; the poll is the backstop). When that window goes (the viewer made
/// a new one), "Starting" shows and the next window goes in. Until then the `concealer` keeps the
/// viewer's windows out of sight. A window that never appears, or cannot be embedded, is given back in sight and left
/// to the viewer (the display window goes): see [`settle_viewer_window`].
#[cfg(windows)]
async fn embed_viewer_window(
    pid: u32,
    host: Arc<dyn ViewerHost>,
    concealer: Arc<std::sync::Mutex<Option<crate::winwindow::Concealer>>>,
    found: Arc<tokio::sync::Notify>,
    streaming: Arc<std::sync::atomic::AtomicBool>,
    placement: Placement,
    stream_monitor: Option<u32>,
    full_screen: bool,
) {
    use crate::winwindow;

    // The fallback: the viewer's windows back, and the one it speaks in (its launcher, with
    // whatever it says) brought to the user over the display window, in front, while the display
    // window is still there to hand the front over.
    let give_back = {
        let host = Arc::clone(&host);
        move || {
            let taken = concealer.lock().unwrap().take();
            if let Some(concealer) = taken {
                concealer.reveal();
            }
            // Only while no stream window is there: with one, the viewer streams in its own.
            if winwindow::find_stream_window_of_pid(pid).is_none() {
                if let Some(window) = winwindow::talking_window_of(pid) {
                    winwindow::bring_to_user(window, host.area());
                }
            }
        }
    };
    loop {
        let looking = tokio::time::Instant::now();
        let mut viewer_shown = false;
        let window = loop {
            tokio::select! {
                _ = tokio::time::sleep(EMBED_POLL) => {}
                _ = found.notified() => {}
            }
            let waited = looking.elapsed().as_millis() as u64;
            let now = Looking { waited_ms: waited, viewer_shown, own_window_shows: winwindow::shows_own_window(pid) };
            match embed_plan(winwindow::find_stream_window_of_pid(pid), now) {
                EmbedPlan::Embed(window) => break window,
                EmbedPlan::Wait => continue,
                EmbedPlan::ShowViewer => {
                    viewer_shown = true;
                    crate::trace::line(|| format!("viewer {pid} no stream after {waited} ms: its own window to the user"));
                    if let Some(window) = winwindow::talking_window_of(pid) {
                        winwindow::bring_to_user(window, host.area());
                    }
                }
                EmbedPlan::Fallback => {
                    eprintln!("expand: no viewer window to embed after {waited} ms; it keeps its own");
                    crate::trace::line(|| format!("viewer {pid} fallback: none to embed after {waited} ms"));
                    give_back();
                    host.fallback();
                    return settle_viewer_window(pid, placement, stream_monitor, full_screen).await;
                }
            }
        };
        // The viewer's own window brought up while it was slow goes again, before the stream
        // goes in and the display window takes the front (the viewer hides it for its stream;
        // it may not, having shown it before we did).
        if viewer_shown {
            if let Some(talking) = winwindow::talking_window_of(pid).filter(|w| winwindow::is_visible(*w)) {
                winwindow::hide_soon(talking);
            }
        }
        let what = winwindow::describe(window);
        crate::trace::line(|| format!("viewer {pid} embedding {what}"));
        let attaching = Arc::clone(&host);
        let attached = tokio::task::spawn_blocking(move || attaching.attach(window))
            .await
            .unwrap_or_else(|e| Err(e.to_string()));
        if let Err(e) = attached {
            eprintln!("expand: could not embed the viewer window {what}: {e}; it keeps its own");
            give_back();
            host.fallback();
            return settle_viewer_window(pid, placement, stream_monitor, full_screen).await;
        }
        eprintln!("expand: embedded the viewer window {what}");
        crate::trace::line(|| format!("viewer {pid} embedded {what}"));
        winwindow::set_embedded_viewer(Some(window));
        streaming.store(true, std::sync::atomic::Ordering::Relaxed);
        // The display window forgets it the moment it is destroyed (it hears of it from
        // Windows); this poll is the backstop.
        while winwindow::is_window(window) {
            tokio::time::sleep(EMBED_POLL).await;
        }
        streaming.store(false, std::sync::atomic::Ordering::Relaxed);
        winwindow::set_embedded_viewer(None);
        host.detach();
    }
}

/// Gives the viewer window our title and icon, and puts it on the chosen monitor, or the one next to the Mac.
/// The viewer sets its own title, and may move itself, a little after its window appears, so
/// this keeps at it for a while. It moves the window only once: after that it is the user's.
#[cfg(windows)]
async fn settle_viewer_window(pid: u32, placement: Placement, stream_monitor: Option<u32>, full_screen: bool) {
    use crate::layout::{is_on, viewer_frame, viewer_monitor};
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
            (viewer_monitor(&winwindow::monitors(), stream_monitor, &placement), winwindow::window_rect(window))
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
const VIEWER_TITLE: &str = "Expand";

#[cfg(target_os = "macos")]
pub use mac::{cleanup_orphans, RealHostEngines};

#[cfg(target_os = "macos")]
mod mac {
    use crate::macdisplay;
    use crate::session::HostEngines;
    use crate::side::Side;
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
        // By full path: process names are cut to 16 characters, which "expand-display" is not.
        quiet("pkill", &["-f", "/expand-display "]).await;
    }

    /// The virtual display and Sunshine on the Mac.
    pub struct RealHostEngines {
        data_dir: PathBuf,
        user: String,
        pass: String,
        host: Option<Host>,
        display: Option<macdisplay::VirtualDisplay>,
        pin_attempts: u32,
        /// Which side of the Mac the PC is on, asked for when the display is placed (the PC
        /// announces its arrangement just before the stream starts).
        toward_pc: Box<dyn Fn() -> Side + Send>,
    }

    impl RealHostEngines {
        pub fn new(
            data_dir: PathBuf,
            user: String,
            pass: String,
            toward_pc: impl Fn() -> Side + Send + 'static,
        ) -> RealHostEngines {
            RealHostEngines {
                data_dir,
                user,
                pass,
                host: None,
                display: None,
                pin_attempts: 0,
                toward_pc: Box::new(toward_pc),
            }
        }

        async fn bring_up(&mut self, width: u32, height: u32, fps: u32, toward_pc: Side) -> Result<(), EngineError> {
            self.pin_attempts = 0;
            let exe = sunshine::find_exe().ok_or(EngineError::NotInstalled("Sunshine"))?;
            tokio::fs::create_dir_all(&self.data_dir).await.map_err(EngineError::Io)?;
            cleanup_orphans(&self.data_dir).await;
            if sunshine::is_port_in_use().await {
                return Err(EngineError::AlreadyRunning("Sunshine"));
            }

            // Exactly the size the PC asked for; its ID is new every time.
            let display = self.display.insert(macdisplay::create_virtual(width, height, fps).await?).id;
            // Beside the MacBook, on the PC's side. A display that stays put still works; only
            // dragging across suffers, so this never fails.
            if let Err(e) = macdisplay::place_virtual(display, toward_pc) {
                eprintln!("expand: {e}");
            }

            let conf = sunshine::conf_path(&self.data_dir);
            tokio::fs::write(&conf, sunshine::conf_text(&display.to_string(), &self.data_dir))
                .await
                .map_err(EngineError::Io)?;
            sunshine::set_credentials(&exe, &conf, &self.user, &self.pass).await?;

            let host = self.host.insert(Host::spawn(&exe, &conf, &sunshine::log_path(&self.data_dir)).await?);
            host.wait_ready(SUNSHINE_WAIT).await?;
            // Ctrl as ⌘ in the display window rewrites only what this Sunshine types.
            crate::keys_stream::follow_sunshine(host.pid());
            Ok(())
        }
    }

    #[async_trait]
    impl HostEngines for RealHostEngines {
        async fn start(&mut self, width: u32, height: u32, fps: u32) -> Result<(), EngineError> {
            let toward_pc = (self.toward_pc)();
            let result = self.bring_up(width, height, fps, toward_pc).await;
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
            crate::keys_stream::follow_sunshine(None);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn looking(waited_ms: u64) -> Looking {
        Looking { waited_ms, viewer_shown: false, own_window_shows: false }
    }

    #[test]
    fn a_found_window_is_embedded() {
        assert_eq!(embed_plan(Some(42), looking(0)), EmbedPlan::Embed(42));
        assert_eq!(embed_plan(Some(42), looking(20_000)), EmbedPlan::Embed(42));
        assert_eq!(embed_plan(Some(42), Looking { viewer_shown: true, ..looking(60_000) }), EmbedPlan::Embed(42));
    }

    #[test]
    fn no_window_yet_means_wait() {
        assert_eq!(embed_plan(None, looking(0)), EmbedPlan::Wait);
        assert_eq!(embed_plan(None, looking(14_999)), EmbedPlan::Wait);
    }

    #[test]
    fn no_stream_after_15_seconds_shows_the_viewer_over_the_display_and_keeps_waiting() {
        // The viewer is still connecting, or says why it can't: its launcher comes to the user,
        // over "Starting"; a stream that still comes goes in, an error ends the session.
        assert_eq!(embed_plan(None, looking(15_000)), EmbedPlan::ShowViewer);
        assert_eq!(embed_plan(None, Looking { viewer_shown: true, ..looking(15_000) }), EmbedPlan::Wait);
        assert_eq!(embed_plan(None, Looking { viewer_shown: true, ..looking(600_000) }), EmbedPlan::Wait);
    }

    #[test]
    fn a_viewer_showing_its_stream_in_a_window_not_found_keeps_its_own() {
        // Its stream in a window of a class not known as the stream's: it keeps that window.
        assert_eq!(embed_plan(None, Looking { own_window_shows: true, ..looking(15_000) }), EmbedPlan::Fallback);
        assert_eq!(embed_plan(None, Looking { own_window_shows: true, ..looking(14_000) }), EmbedPlan::Wait);
    }
}
