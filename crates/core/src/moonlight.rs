//! The viewer on the PC: Moonlight, launched with settings the app fixes.
//! Option names checked against moonlight-qt's commandlineparser.cpp on 2026-11-11.

use crate::config::StreamSettings;
use std::path::{Path, PathBuf};
use std::process::Stdio;
#[cfg(not(windows))]
use tokio::process::Child;
use tokio::process::Command;

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

/// Moonlight always streams in a window of its own: Sidescreen's display window holds it, and
/// `window_mode` says whether that window fills a monitor.
/// Whether the Mac scrolls the other way round in the display (the PC's Reverse scrolling
/// setting, on by default; the app keeps it up to date with [`set_reverse_scroll`]).
static REVERSE_SCROLL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// The Reverse scrolling setting, for the next stream started ([`Viewer::spawn`]).
pub fn set_reverse_scroll(on: bool) {
    REVERSE_SCROLL.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// What the stream starts with. `reverse_scroll` is said either way, so Moonlight's own saved
/// choice never decides.
pub fn stream_args(host: &str, s: &StreamSettings, reverse_scroll: bool) -> Vec<String> {
    [
        "stream", host, APP_NAME,
        "--resolution", &format!("{}x{}", s.width, s.height),
        "--fps", &s.fps.to_string(),
        "--bitrate", &s.bitrate_kbps.to_string(),
        "--display-mode", "windowed",
        "--absolute-mouse",
        "--capture-system-keys", "never",
        if reverse_scroll { "--reverse-scroll-direction" } else { "--no-reverse-scroll-direction" },
    ]
    .iter()
    .map(|a| a.to_string())
    .collect()
}

/// The environment the stream runs with. SDL's hint (read from the environment ahead of the
/// app's own settings) lets a click that gives the stream the keyboard through to it, so the first
/// click on the stream after using the title bar reaches the Mac.
pub fn stream_env() -> [(&'static str, &'static str); 1] {
    [("SDL_MOUSE_FOCUS_CLICKTHROUGH", "1")]
}

pub fn pair_args(host: &str, pin: &str) -> Vec<String> {
    ["pair", host, "--pin", pin].iter().map(|a| a.to_string()).collect()
}

pub fn list_args(host: &str) -> Vec<String> {
    ["list", host].iter().map(|a| a.to_string()).collect()
}

/// What moonlight-qt's `list` prints (to stderr, then exits -1) for a host it is not paired with.
const NOT_PAIRED: &str = "has not been paired";

/// Reads the outcome of `list`: `Some(true)` paired, `Some(false)` not paired, `None` unknown
/// (unreachable, or an answer we do not recognise).
pub fn paired_from_list(exit_code: Option<i32>, stderr: &str) -> Option<bool> {
    if exit_code == Some(0) {
        Some(true)
    } else if stderr.contains(NOT_PAIRED) {
        Some(false)
    } else {
        None
    }
}

/// Whether the stream's viewer gets the in-context hook (`sidescreen_viewer_hook.dll`, which
/// keeps its stream window out of sight from the moment it is made): unless
/// `SIDESCREEN_VIEWER_HOOK=0` (`hook`) or `SIDESCREEN_VIEWER_PLAIN=1` (`plain`), and only for a
/// viewer built for the same machine as this app (`viewer_machine`, from [`exe_machine`]: our
/// library can't go into another). Without it, the window is moved from outside, a moment late.
pub fn viewer_hook_wanted(hook: Option<&std::ffi::OsStr>, plain: Option<&std::ffi::OsStr>, viewer_machine: Option<u16>) -> bool {
    !hook.is_some_and(|v| v == "0") && !plain.is_some_and(|v| v == "1") && viewer_machine.is_some() && viewer_machine == own_machine()
}

/// The machine this app is built for, as a PE file's machine type.
pub fn own_machine() -> Option<u16> {
    if cfg!(target_arch = "x86_64") {
        Some(0x8664)
    } else if cfg!(target_arch = "aarch64") {
        Some(0xAA64)
    } else if cfg!(target_arch = "x86") {
        Some(0x014C)
    } else {
        None
    }
}

/// The machine type in a Windows program's header (its first bytes): `0x8664` x64, `0xAA64`
/// ARM64, `0x014C` x86; `None` when it is not a PE file.
pub fn exe_machine(head: &[u8]) -> Option<u16> {
    if head.get(..2)? != b"MZ" {
        return None;
    }
    let at = u32::from_le_bytes(head.get(0x3C..0x40)?.try_into().ok()?) as usize;
    if head.get(at..at + 4)? != b"PE\0\0" {
        return None;
    }
    Some(u16::from_le_bytes(head.get(at + 4..at + 6)?.try_into().ok()?))
}

/// The machine type of the program at `exe` ([`exe_machine`], from its first 4 KB).
pub fn machine_of(exe: &Path) -> Option<u16> {
    use std::io::Read;
    let mut head = vec![0u8; 4096];
    let n = std::fs::File::open(exe).and_then(|mut f| f.read(&mut head)).ok()?;
    exe_machine(&head[..n])
}

/// Whether an error Moonlight put up says it is not paired with the stream host (it then pairs
/// again next time).
pub fn says_unpaired(said: &str) -> bool {
    let said = said.to_lowercase();
    said.contains("not been paired") || said.contains("not paired") || said.contains("unpaired")
}

/// How long `list` may take. With the stream host up it answers in a moment; it gives up on an
/// unreachable host only after 30 seconds, which this cuts short.
const LIST_WAIT: std::time::Duration = std::time::Duration::from_secs(15);

/// Asks Moonlight whether it is paired with `host`. `None` when it cannot tell in time. The
/// process it runs for that is told to `started` as it starts.
pub async fn is_paired(exe: &Path, host: &str, started: impl FnOnce(u32)) -> std::io::Result<Option<bool>> {
    let mut cmd = Command::new(exe);
    cmd.args(list_args(host)).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).kill_on_drop(true);
    no_console(&mut cmd);
    let child = cmd.spawn()?;
    if let Some(pid) = child.id() {
        crate::trace::line(|| format!("moonlight list started, pid {pid} (no window of its own)"));
        started(pid);
    }
    match tokio::time::timeout(LIST_WAIT, child.wait_with_output()).await {
        Ok(out) => {
            let out = out?;
            Ok(paired_from_list(out.status.code(), &String::from_utf8_lossy(&out.stderr)))
        }
        Err(_) => Ok(None),
    }
}

/// A run that only answers (`list`) never gets a console window: Moonlight is a windowed program,
/// so this is only a guard should it ever ask for one.
fn no_console(_cmd: &mut Command) {
    #[cfg(windows)]
    _cmd.creation_flags(CREATE_NO_WINDOW);
}

/// `CREATE_NO_WINDOW`.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[cfg(not(windows))]
fn command(exe: &Path, args: Vec<String>) -> Command {
    let mut cmd = Command::new(exe);
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).kill_on_drop(true);
    cmd
}

/// A running viewer window.
pub struct Viewer {
    #[cfg(windows)]
    child: hidden::Child,
    #[cfg(not(windows))]
    child: Child,
    /// What it writes to its error output: its log (Windows).
    #[cfg(windows)]
    log: Option<std::fs::File>,
}

/// Whether the stream starts hidden (its launcher never shows, [`Viewer::spawn`]): only when its
/// windows go into the display window (`embedding`), which is what shows the user what happens
/// (else the viewer is all there is to see), and unless `SIDESCREEN_VIEWER_PLAIN=1` (`plain`; dev
/// only, to compare with the plain start in one live run).
pub fn stream_starts_hidden(embedding: bool, plain: Option<&std::ffi::OsStr>) -> bool {
    embedding && !plain.is_some_and(|v| v == "1")
}

impl Viewer {
    /// Starts the stream. On Windows it starts hidden ([`hidden::Child`]): the first window it
    /// shows is the launcher it puts up while the stream starts, which then never shows (seen
    /// 2026-11-24 with Moonlight 6.2: a plain start shows it at once, and its thread is too busy
    /// for ~300 ms to be moved out of sight). Its stream window shows later, as usual (a hidden
    /// start hides only the first window shown), and goes into the display window.
    /// `hidden`: [`stream_starts_hidden`]. On Windows its log (its error output) is kept for
    /// [`take_log`](Viewer::take_log).
    pub fn spawn(exe: &Path, host: &str, s: &StreamSettings, hidden: bool) -> std::io::Result<Viewer> {
        #[cfg(windows)]
        let viewer = {
            let how = hidden::Start { env: &stream_env(), hide_first: hidden, log: true };
            let (child, log) = hidden::Child::start(exe, &stream_args(host, s, REVERSE_SCROLL.load(std::sync::atomic::Ordering::Relaxed)), &how)?;
            Viewer { child, log }
        };
        #[cfg(not(windows))]
        let viewer = {
            let _ = hidden;
            let mut cmd = command(exe, stream_args(host, s, REVERSE_SCROLL.load(std::sync::atomic::Ordering::Relaxed)));
            cmd.envs(stream_env());
            Viewer { child: cmd.spawn()? }
        };
        crate::trace::line(|| {
            format!("moonlight stream started{}, pid {}", if hidden { " hidden" } else { "" }, viewer.pid().unwrap_or(0))
        });
        Ok(viewer)
    }

    /// Its log, to read as it is written (once; Windows).
    #[cfg(windows)]
    pub fn take_log(&mut self) -> Option<std::fs::File> {
        self.log.take()
    }

    pub fn pid(&self) -> Option<u32> {
        #[cfg(windows)]
        return Some(self.child.pid());
        #[cfg(not(windows))]
        return self.child.id();
    }

    /// Resolves when the viewer window is closed.
    pub async fn wait(&mut self) -> std::io::Result<()> {
        self.child.wait().await.map(|_| ())
    }

    pub async fn kill(&mut self) {
        #[cfg(windows)]
        {
            self.child.kill();
            let _ = self.child.wait().await;
        }
        #[cfg(not(windows))]
        let _ = self.child.kill().await;
    }
}

/// Runs Moonlight's pairing with a PIN we chose. True when it reports success. The process it
/// runs for that is told to `started` as it starts.
/// On Windows it starts hidden (`STARTUPINFO` asks for `SW_HIDE`, which Windows uses for the
/// first window it shows): the pairing needs nothing from the user (the PIN is ours), and its
/// dialog never shows. Killed when dropped.
pub async fn pair(exe: &Path, host: &str, pin: &str, started: impl FnOnce(u32)) -> std::io::Result<bool> {
    #[cfg(windows)]
    {
        let child = hidden::Child::spawn(exe, &pair_args(host, pin))?;
        // Never the PIN.
        crate::trace::line(|| format!("moonlight pair started hidden, pid {}", child.pid()));
        started(child.pid());
        Ok(child.wait().await? == 0)
    }
    #[cfg(not(windows))]
    {
        let mut child = command(exe, pair_args(host, pin)).spawn()?;
        if let Some(pid) = child.id() {
            started(pid);
        }
        Ok(child.wait().await?.success())
    }
}

/// One argument as Windows programs read it back from their command line (the rules of
/// `CommandLineToArgvW`): as it is when it has no space, tab or quote, else quoted, with the
/// backslashes before a quote (and at the end) doubled.
pub fn quote_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        return arg.to_string();
    }
    let mut out = String::from('"');
    let mut slashes = 0;
    for c in arg.chars() {
        match c {
            '\\' => slashes += 1,
            '"' => {
                out.extend(std::iter::repeat_n('\\', slashes * 2 + 1));
                out.push('"');
                slashes = 0;
            }
            _ => {
                out.extend(std::iter::repeat_n('\\', slashes));
                out.push(c);
                slashes = 0;
            }
        }
    }
    out.extend(std::iter::repeat_n('\\', slashes * 2));
    out.push('"');
    out
}

/// An environment block for `CreateProcessW` (UTF-16, `CREATE_UNICODE_ENVIRONMENT`): `vars` with
/// `extra` set over them (names match whatever their case), sorted by name ignoring case, as
/// Windows wants it, each `name=value` ended by a 0, and a 0 after the last.
pub fn environment_block(vars: impl IntoIterator<Item = (String, String)>, extra: &[(&str, &str)]) -> Vec<u16> {
    let mut all: Vec<(String, String)> = vars
        .into_iter()
        .filter(|(k, _)| !extra.iter().any(|(e, _)| e.eq_ignore_ascii_case(k)))
        .chain(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())))
        .collect();
    all.sort_by_key(|(k, _)| k.to_uppercase());
    let mut block: Vec<u16> = Vec::new();
    for (k, v) in all {
        block.extend(format!("{k}={v}").encode_utf16());
        block.push(0);
    }
    if block.is_empty() {
        block.push(0);
    }
    block.push(0);
    block
}

/// A whole command line: the program, then its arguments, each [`quote_arg`]ed.
pub fn command_line(exe: &str, args: &[String]) -> String {
    std::iter::once(quote_arg(exe)).chain(args.iter().map(|a| quote_arg(a))).collect::<Vec<_>>().join(" ")
}

/// A process started hidden, which std's `Command` can't ask for on stable Rust.
#[cfg(windows)]
pub mod hidden {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::path::Path;
    use std::sync::Arc;
    use windows::core::PWSTR;
    use windows::Win32::Foundation::{SetHandleInformation, HANDLE, HANDLE_FLAGS, HANDLE_FLAG_INHERIT, WAIT_OBJECT_0};
    use windows::Win32::Security::SECURITY_ATTRIBUTES;
    use windows::Win32::System::Pipes::CreatePipe;
    use windows::Win32::System::Threading::{
        CreateProcessW, GetExitCodeProcess, TerminateProcess, WaitForSingleObject, CREATE_NO_WINDOW,
        CREATE_UNICODE_ENVIRONMENT, INFINITE, PROCESS_INFORMATION, STARTF_USESHOWWINDOW, STARTF_USESTDHANDLES,
        STARTUPINFOW,
    };
    use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;

    /// How a [`Child`] starts.
    pub struct Start<'a> {
        /// Set over our environment.
        pub env: &'a [(&'a str, &'a str)],
        /// The first window it shows stays hidden (`STARTUPINFO` with `SW_HIDE`; later ones show
        /// as they would).
        pub hide_first: bool,
        /// Its output and error output come back through a pipe, to read as it writes.
        pub log: bool,
    }

    /// A running process; killed when dropped while it still runs.
    pub struct Child {
        process: Arc<OwnedHandle>,
        pid: u32,
    }

    fn raw(h: &OwnedHandle) -> HANDLE {
        HANDLE(h.as_raw_handle())
    }

    impl Child {
        /// Starts `exe` with `args`: the first window it shows stays hidden (later ones show as
        /// they would), no console, none of our handles, our environment.
        pub fn spawn(exe: &Path, args: &[String]) -> std::io::Result<Child> {
            Child::spawn_with(exe, args, &[])
        }

        /// [`spawn`](Child::spawn), with `env` set over our environment.
        pub fn spawn_with(exe: &Path, args: &[String], env: &[(&str, &str)]) -> std::io::Result<Child> {
            Child::start(exe, args, &Start { env, hide_first: true, log: false }).map(|(child, _)| child)
        }

        /// Starts `exe` with `args` as `how` says, with no console and, but for its log, none of
        /// our handles. Returns its log to read when `how.log` (the reader must keep reading, or
        /// the program stalls once the pipe is full).
        pub fn start(exe: &Path, args: &[String], how: &Start) -> std::io::Result<(Child, Option<std::fs::File>)> {
            let vars = std::env::vars_os().map(|(k, v)| (k.to_string_lossy().into_owned(), v.to_string_lossy().into_owned()));
            let block = super::environment_block(vars, how.env);
            let line = super::command_line(&exe.to_string_lossy(), args);
            let mut wide: Vec<u16> = line.encode_utf16().chain(std::iter::once(0)).collect();
            let mut startup = STARTUPINFOW { cb: std::mem::size_of::<STARTUPINFOW>() as u32, ..Default::default() };
            if how.hide_first {
                startup.dwFlags |= STARTF_USESHOWWINDOW;
                startup.wShowWindow = SW_HIDE.0 as u16;
            }
            unsafe {
                // The log: a pipe whose writing end only the child inherits.
                let mut pipe: Option<(OwnedHandle, OwnedHandle)> = None;
                if how.log {
                    let (mut read, mut write) = (HANDLE::default(), HANDLE::default());
                    let inherit = SECURITY_ATTRIBUTES {
                        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                        bInheritHandle: true.into(),
                        ..Default::default()
                    };
                    CreatePipe(&mut read, &mut write, Some(&inherit), 0).map_err(|e| std::io::Error::other(e.message()))?;
                    let (read, write) = (OwnedHandle::from_raw_handle(read.0), OwnedHandle::from_raw_handle(write.0));
                    SetHandleInformation(raw(&read), HANDLE_FLAG_INHERIT.0, HANDLE_FLAGS(0))
                        .map_err(|e| std::io::Error::other(e.message()))?;
                    startup.dwFlags |= STARTF_USESTDHANDLES;
                    startup.hStdOutput = raw(&write);
                    startup.hStdError = raw(&write);
                    pipe = Some((read, write));
                }
                let mut info = PROCESS_INFORMATION::default();
                let flags = CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT;
                let env = Some(block.as_ptr() as *const core::ffi::c_void);
                CreateProcessW(None, PWSTR(wide.as_mut_ptr()), None, None, how.log, flags, env, None, &startup, &mut info)
                    .map_err(|e| std::io::Error::other(e.message()))?;
                drop(OwnedHandle::from_raw_handle(info.hThread.0));
                let child = Child { process: Arc::new(OwnedHandle::from_raw_handle(info.hProcess.0)), pid: info.dwProcessId };
                // Our copy of the writing end goes, so the log ends when the child does.
                let log = pipe.map(|(read, _write)| std::fs::File::from(read));
                Ok((child, log))
            }
        }

        pub fn pid(&self) -> u32 {
            self.pid
        }

        /// Its exit code, once it ends. Waits on a blocking thread; if the child is dropped
        /// meanwhile, the process is killed and that thread ends with it.
        pub async fn wait(&self) -> std::io::Result<u32> {
            let process = Arc::clone(&self.process);
            tokio::task::spawn_blocking(move || unsafe {
                WaitForSingleObject(raw(&process), INFINITE);
                let mut code = 0u32;
                GetExitCodeProcess(raw(&process), &mut code).map_err(|e| std::io::Error::other(e.message()))?;
                Ok(code)
            })
            .await
            .map_err(std::io::Error::other)?
        }

        /// Ends it now, unless it ended already.
        pub fn kill(&self) {
            if self.running() {
                unsafe {
                    let _ = TerminateProcess(raw(&self.process), 1);
                }
            }
        }

        fn running(&self) -> bool {
            unsafe { WaitForSingleObject(raw(&self.process), 0) != WAIT_OBJECT_0 }
        }
    }

    impl Drop for Child {
        fn drop(&mut self) {
            self.kill();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WindowMode;

    fn settings(mode: WindowMode) -> StreamSettings {
        StreamSettings { width: 2560, height: 1440, fps: 60, bitrate_kbps: 30000, window_mode: mode, monitor: None }
    }

    #[test]
    fn stream_args_windowed() {
        assert_eq!(
            stream_args("192.168.1.50", &settings(WindowMode::Windowed), false),
            [
                "stream", "192.168.1.50", "Desktop",
                "--resolution", "2560x1440", "--fps", "60", "--bitrate", "30000",
                "--display-mode", "windowed",
                "--absolute-mouse", "--capture-system-keys", "never",
                "--no-reverse-scroll-direction",
            ]
        );
    }

    /// Reverse scrolling (the PC's setting, on by default): the Mac scrolls the other way round in
    /// the display window too. Said either way, so Moonlight's own saved choice never decides.
    #[test]
    fn the_stream_scrolls_the_way_the_setting_says() {
        let on = stream_args("h", &settings(WindowMode::Windowed), true);
        assert!(on.contains(&"--reverse-scroll-direction".to_string()));
        assert!(!on.contains(&"--no-reverse-scroll-direction".to_string()));
        assert!(stream_args("h", &settings(WindowMode::Windowed), false).contains(&"--no-reverse-scroll-direction".to_string()));
    }

    #[test]
    fn full_screen_still_streams_in_a_window() {
        // The display window goes full screen around it.
        let args = stream_args("h", &settings(WindowMode::Borderless), false);
        let i = args.iter().position(|a| a == "--display-mode").unwrap();
        assert_eq!(args[i + 1], "windowed");
    }

    #[test]
    fn the_stream_lets_the_click_that_focuses_it_through() {
        assert!(stream_env().contains(&("SDL_MOUSE_FOCUS_CLICKTHROUGH", "1")));
    }

    #[test]
    fn list_args_shape() {
        assert_eq!(list_args("192.168.1.50"), ["list", "192.168.1.50"]);
    }

    // What moonlight-qt's `list` gives (cli/listapps.cpp; the strings checked in the installed
    // Moonlight.exe on 2026-11-23): the app names and exit 0 when paired; this message and exit -1
    // when it is not; "Failed to connect to <host>" and exit -1 after 30 s when it cannot reach it.
    #[test]
    fn a_successful_list_means_paired() {
        assert_eq!(paired_from_list(Some(0), ""), Some(true));
    }

    #[test]
    fn the_not_paired_message_means_not_paired() {
        let says = "Computer Mac.lan has not been paired. Please open Moonlight to pair before retrieving games list.
";
        assert_eq!(paired_from_list(Some(-1), says), Some(false));
        // The message decides, whatever the exit code reads as on this system.
        assert_eq!(paired_from_list(Some(255), says), Some(false));
        assert_eq!(paired_from_list(None, says), Some(false));
    }

    #[test]
    fn anything_else_is_unknown() {
        assert_eq!(paired_from_list(Some(-1), "Failed to connect to 192.168.1.50
"), None);
        assert_eq!(paired_from_list(Some(1), "error: timed out"), None);
        assert_eq!(paired_from_list(Some(-1), ""), None);
        assert_eq!(paired_from_list(None, ""), None);
    }

    #[test]
    fn plain_arguments_go_as_they_are() {
        assert_eq!(quote_arg("pair"), "pair");
        assert_eq!(quote_arg("192.168.1.50"), "192.168.1.50");
        assert_eq!(quote_arg("fe80::1%12"), "fe80::1%12");
    }

    #[test]
    fn arguments_with_spaces_or_quotes_are_quoted_as_windows_reads_them() {
        assert_eq!(quote_arg(r"C:\Program Files\Moonlight.exe"), r#""C:\Program Files\Moonlight.exe""#);
        assert_eq!(quote_arg(""), r#""""#);
        assert_eq!(quote_arg(r#"a"b"#), r#""a\"b""#);
        assert_eq!(quote_arg(r#"a\"b"#), r#""a\\\"b""#);
        // Backslashes before the closing quote are doubled; elsewhere they stay.
        assert_eq!(quote_arg(r"a b\"), r#""a b\\""#);
        assert_eq!(quote_arg(r"a\b c"), r#""a\b c""#);
    }

    #[test]
    fn a_command_line_is_the_program_then_its_arguments() {
        let args = ["pair".to_string(), "host".to_string(), "--pin".to_string(), "0042".to_string()];
        assert_eq!(command_line(r"C:\M L\Moonlight.exe", &args), r#""C:\M L\Moonlight.exe" pair host --pin 0042"#);
    }

    #[test]
    fn the_environment_is_ours_with_the_extra_set_over_it_sorted_ignoring_case() {
        let ours = [("b".to_string(), "2".to_string()), ("SDL_X".to_string(), "old".to_string()), ("A".to_string(), "1".to_string())];
        let block = environment_block(ours, &[("sdl_x", "1")]);
        let text = String::from_utf16(&block).unwrap();
        assert_eq!(text, "A=1\0b=2\0sdl_x=1\0\0");
    }

    #[test]
    fn an_empty_environment_is_two_zeros() {
        assert_eq!(environment_block(Vec::new(), &[]), vec![0, 0]);
    }

    /// The extra variables reach the program started hidden.
    #[cfg(windows)]
    #[tokio::test]
    async fn a_hidden_process_gets_the_extra_environment() {
        let args: Vec<String> = ["/c", "if", "%SIDESCREEN_PROBE%==yes", "(exit", "7)", "else", "(exit", "1)"].iter().map(|a| a.to_string()).collect();
        let child = hidden::Child::spawn_with(&cmd_exe(), &args, &[("SIDESCREEN_PROBE", "yes")]).unwrap();
        assert_eq!(child.wait().await.unwrap(), 7);
    }

    #[test]
    fn the_stream_starts_hidden_only_into_the_display_window_and_unless_asked_plain() {
        assert!(stream_starts_hidden(true, None));
        assert!(stream_starts_hidden(true, Some("0".as_ref())));
        assert!(!stream_starts_hidden(true, Some("1".as_ref())));
        // Not embedding: the viewer's own windows are all there is to see.
        assert!(!stream_starts_hidden(false, None));
    }

    /// What a program started with its log kept writes comes back, and the log ends with it.
    #[cfg(windows)]
    #[tokio::test]
    async fn a_hidden_process_log_is_read_as_it_writes() {
        use std::io::Read;
        let args: Vec<String> = ["/c", "echo", "one&&", "echo", "two", "1>&2"].iter().map(|a| a.to_string()).collect();
        let how = hidden::Start { env: &[], hide_first: true, log: true };
        let (child, log) = hidden::Child::start(&cmd_exe(), &args, &how).unwrap();
        let mut text = String::new();
        log.unwrap().read_to_string(&mut text).unwrap();
        assert_eq!(child.wait().await.unwrap(), 0);
        assert_eq!(text.split_whitespace().collect::<Vec<_>>(), ["one", "two"]);
    }

    #[cfg(windows)]
    fn cmd_exe() -> std::path::PathBuf {
        std::env::var_os("ComSpec").map(Into::into).unwrap_or_else(|| r"C:\Windows\System32\cmd.exe".into())
    }

    /// A program started hidden runs, and gives its exit code (no console is made for cmd).
    #[cfg(windows)]
    #[tokio::test]
    async fn a_hidden_process_runs_and_gives_its_exit_code() {
        let args: Vec<String> = ["/c", "exit", "3"].iter().map(|a| a.to_string()).collect();
        let child = hidden::Child::spawn(&cmd_exe(), &args).unwrap();
        assert!(child.pid() != 0);
        assert_eq!(child.wait().await.unwrap(), 3);
    }

    /// Dropped while it runs, it is killed.
    #[cfg(windows)]
    #[tokio::test]
    async fn a_hidden_process_dropped_is_killed() {
        use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
        use windows::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};
        let args: Vec<String> = ["/c", "ping", "-n", "30", "127.0.0.1"].iter().map(|a| a.to_string()).collect();
        let child = hidden::Child::spawn(&cmd_exe(), &args).unwrap();
        let process = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, child.pid()) }.unwrap();
        drop(child);
        let ended = unsafe { WaitForSingleObject(process, 3000) } == WAIT_OBJECT_0;
        unsafe {
            let _ = CloseHandle(process);
        }
        assert!(ended, "the process was left running");
    }

    /// A PE header with this machine type.
    fn pe(machine: u16) -> Vec<u8> {
        let mut head = vec![0u8; 0x200];
        head[..2].copy_from_slice(b"MZ");
        head[0x3C..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        head[0x80..0x84].copy_from_slice(b"PE\0\0");
        head[0x84..0x86].copy_from_slice(&machine.to_le_bytes());
        head
    }

    #[test]
    fn a_programs_machine_is_read_from_its_header() {
        assert_eq!(exe_machine(&pe(0x8664)), Some(0x8664));
        assert_eq!(exe_machine(&pe(0xAA64)), Some(0xAA64));
        assert_eq!(exe_machine(b"not a program"), None);
        assert_eq!(exe_machine(&pe(0x8664)[..0x84]), None);
        assert_eq!(exe_machine(b""), None);
    }

    /// This test program is built for the machine the app is.
    #[cfg(windows)]
    #[test]
    fn this_program_is_of_our_own_machine() {
        assert_eq!(machine_of(&std::env::current_exe().unwrap()), own_machine());
    }

    #[test]
    fn the_hook_goes_into_a_viewer_of_our_machine_unless_turned_off() {
        let ours = own_machine();
        assert!(viewer_hook_wanted(None, None, ours));
        assert!(viewer_hook_wanted(Some("1".as_ref()), Some("0".as_ref()), ours));
        assert!(!viewer_hook_wanted(Some("0".as_ref()), None, ours));
        assert!(!viewer_hook_wanted(None, Some("1".as_ref()), ours));
        // A 32-bit or ARM64 Moonlight on this x64 app (or the other way), or one not read.
        let other = if ours == Some(0x8664) { 0x014C } else { 0x8664 };
        assert!(!viewer_hook_wanted(None, None, Some(other)));
        assert!(!viewer_hook_wanted(None, None, None));
    }

    #[test]
    fn an_error_saying_it_is_not_paired_is_known() {
        assert!(says_unpaired("Computer Mac.lan has not been paired. Please open Moonlight to pair before streaming."));
        assert!(!says_unpaired("Failed to connect to 192.0.2.1"));
        assert!(!says_unpaired("Your selected video codec isn't supported"));
    }

    #[test]
    fn pair_args_shape() {
        assert_eq!(pair_args("h", "1234"), ["pair", "h", "--pin", "1234"]);
    }
}
