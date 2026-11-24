//! Keeps mouse and keyboard sharing running with the right settings, and reports how it is
//! doing. Sharing is a convenience on top of the display: nothing here ever fails a session.

use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MouseStatus {
    Off,
    /// Reaching the Mac for the first time in this run.
    Connecting,
    /// On the PC `mac_*` name the Mac; on the Mac they name the connected PC. Mac side only:
    /// `uses` is what the PC said it is using the Mac for, `problem` why its mouse and keyboard
    /// cannot be replayed although it is connected (the Accessibility permission is missing), and
    /// `from` the address the PC connected from.
    Connected {
        mac_id: String,
        mac_name: String,
        uses: Option<Uses>,
        problem: Option<String>,
        from: Option<std::net::IpAddr>,
        /// The smoothed round trip between the two, in ms, once measured: measured on the PC,
        /// and on the Mac as the PC reports it.
        rtt_ms: Option<u32>,
    },
    /// The network was lost; retrying.
    Lost(String),
    Ended(EndReason),
}

/// What the connected PC is using the Mac for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Uses {
    pub mouse: bool,
    pub clipboard: bool,
    pub display: bool,
    /// The PC's Ctrl as ⌘ setting, for typing into the display window (`keys_stream`). It
    /// changes nothing about the pace.
    pub ctrl_as_cmd: bool,
}

/// Why a run ended for good (no retry).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndReason {
    ByMac,
    Busy { by: String },
    Blocked,
    Unknown,
    /// The other computer runs a version of Sidescreen this one cannot work with.
    Version,
}

/// What is shared right now on a live connection. Changing it never restarts the connection;
/// the running side reacts to the change.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Toggles {
    pub mouse: bool,
    pub clipboard: bool,
    pub language: bool,
    /// Whether the display is in use; reported to the Mac (`Uses`).
    pub display: bool,
    /// The PC's Ctrl keys reach the Mac as Command, and its Windows keys as Control
    /// (`share::keys`).
    pub ctrl_as_cmd: bool,
    /// The wheel scrolls the Mac the other way round (`wire::Input::scrolled`).
    pub reverse_scroll: bool,
}

/// What a running PC connection follows without reconnecting: what is shared, where the Mac
/// sits, and the stream's size (to find the picture inside the viewer window).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Live {
    pub toggles: Toggles,
    pub placement: crate::config::Placement,
    pub stream: (u32, u32),
}

/// What this machine does in mouse sharing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MouseRole {
    #[cfg(windows)]
    Pc(crate::share::pc::PcSettings),
    #[cfg(target_os = "macos")]
    Mac(crate::share::mac::MacSettings),
}

pub struct MouseManager {
    status: watch::Sender<MouseStatus>,
    #[cfg_attr(not(windows), allow(dead_code))]
    live: watch::Sender<Live>,
    running: Option<(MouseRole, CancellationToken, JoinHandle<()>)>,
}

impl Default for MouseManager {
    fn default() -> Self {
        MouseManager { status: watch::channel(MouseStatus::Off).0, live: watch::channel(Live::default()).0, running: None }
    }
}

impl MouseManager {
    pub fn new() -> MouseManager {
        MouseManager::default()
    }

    pub fn status(&self) -> MouseStatus {
        self.status.borrow().clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<MouseStatus> {
        self.status.subscribe()
    }

    /// Runs sharing in this role. A role with the same identity that is already running is not
    /// restarted: what it follows live (toggles, placement, stream size) is pushed to it.
    pub async fn start(&mut self, role: MouseRole) {
        #[cfg(windows)]
        {
            let MouseRole::Pc(settings) = &role;
            self.live.send_replace(settings.live());
        }
        if self.running.as_ref().is_some_and(|(current, _, task)| *current == role && !task.is_finished()) {
            return;
        }
        self.stop().await;
        let cancel = CancellationToken::new();
        let (stop, status) = (cancel.clone(), self.status.clone());
        // Lan Mouse's capture and emulation must stay on one thread, so sharing gets its own.
        let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();
        let worker = role.clone();
        #[cfg(windows)]
        let live = self.live.subscribe();
        std::thread::spawn(move || {
            let guard_status = status.clone();
            let guard_cancel = stop.clone();
            run_guarded(
                move || {
                    let (worker, status, stop) = (worker.clone(), status.clone(), stop.clone());
                    #[cfg(windows)]
                    let live = live.clone();
                    async move {
                        match worker {
                            #[cfg(windows)]
                            MouseRole::Pc(settings) => crate::share::pc::run(settings, live, status, stop).await,
                            #[cfg(target_os = "macos")]
                            MouseRole::Mac(settings) => crate::share::mac::run(settings, status, stop).await,
                        }
                    }
                },
                &guard_status,
                &guard_cancel,
            );
            let _ = done_tx.send(());
        });
        let task = tokio::spawn(async move {
            let _ = done_rx.await;
        });
        self.running = Some((role, cancel, task));
    }

    pub async fn stop(&mut self) {
        if let Some((_, cancel, task)) = self.running.take() {
            cancel.cancel();
            let _ = task.await;
        }
        self.status.send_replace(MouseStatus::Off);
    }
}

/// Something taken out of a shared slot for a run, put back when the run ends however it ends
/// (a crash included), so the next run finds it there.
pub struct Lent<'a, T> {
    home: &'a std::sync::Mutex<Option<T>>,
    item: Option<T>,
}

impl<'a, T> Lent<'a, T> {
    pub fn take(home: &'a std::sync::Mutex<Option<T>>) -> Lent<'a, T> {
        let item = home.lock().unwrap_or_else(|e| e.into_inner()).take();
        Lent { home, item }
    }

    pub fn item(&mut self) -> &mut Option<T> {
        &mut self.item
    }
}

impl<T> Drop for Lent<'_, T> {
    fn drop(&mut self) {
        *self.home.lock().unwrap_or_else(|e| e.into_inner()) = self.item.take();
    }
}

/// How long to wait before starting sharing again after a crash.
const RESTART_PAUSE: std::time::Duration = std::time::Duration::from_secs(3);

/// Runs sharing on this thread until cancelled, starting it again only after a crash
/// (reported, not silent); a run that returns normally has ended for good. Runs inside a `LocalSet`, which Lan Mouse's code
/// needs for the tasks it starts.
fn run_guarded<F, Fut>(make: F, status: &watch::Sender<MouseStatus>, cancel: &CancellationToken)
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = ()> + 'static,
{
    run_guarded_with_pause(make, status, cancel, RESTART_PAUSE)
}

fn run_guarded_with_pause<F, Fut>(
    make: F,
    status: &watch::Sender<MouseStatus>,
    cancel: &CancellationToken,
    pause: std::time::Duration,
) where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = ()> + 'static,
{
    loop {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a single-thread runtime can always be built");
            tokio::task::LocalSet::new().block_on(&runtime, make());
        }));
        if cancel.is_cancelled() {
            return;
        }
        {
            let Err(payload) = outcome else { return };
            let what = payload
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown error".to_string());
            status.send_replace(MouseStatus::Lost(format!("mouse sharing crashed ({what}); starting it again")));
        }
        // Crashed: try again shortly.
        let until = std::time::Instant::now() + pause;
        while std::time::Instant::now() < until {
            if cancel.is_cancelled() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5).min(pause));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2026-11-18: Lan Mouse's Mac keyboard code starts its key-repeat timer with spawn_local,
    // which panicked outside a LocalSet and silently killed sharing on the first key press.
    #[test]
    fn work_that_spawns_local_tasks_runs() {
        let (status, rx) = watch::channel(MouseStatus::Off);
        let cancel = CancellationToken::new();
        let stop = cancel.clone();
        run_guarded(
            move || {
                let (status, stop) = (status.clone(), stop.clone());
                async move {
                    tokio::task::spawn_local(async {}).await.unwrap();
                    status.send_replace(MouseStatus::Connecting);
                    stop.cancel();
                }
            },
            &watch::channel(MouseStatus::Off).0,
            &cancel,
        );
        assert_eq!(*rx.borrow(), MouseStatus::Connecting);
    }

    #[test]
    fn a_crash_is_reported_and_sharing_restarts() {
        let (status, rx) = watch::channel(MouseStatus::Off);
        let cancel = CancellationToken::new();
        let runs = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let (counter, stop) = (runs.clone(), cancel.clone());
        let seen: std::sync::Arc<std::sync::Mutex<Option<MouseStatus>>> = Default::default();
        let (seen_in, watcher) = (seen.clone(), rx.clone());
        run_guarded_with_pause(
            move || {
                let (counter, stop, seen_in, watcher) = (counter.clone(), stop.clone(), seen_in.clone(), watcher.clone());
                async move {
                    if counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                        panic!("boom");
                    }
                    *seen_in.lock().unwrap() = Some(watcher.borrow().clone());
                    stop.cancel();
                }
            },
            &status,
            &cancel,
            std::time::Duration::from_millis(10),
        );
        assert_eq!(runs.load(std::sync::atomic::Ordering::SeqCst), 2, "restarted once");
        let reported = seen.lock().unwrap().clone();
        match reported {
            Some(MouseStatus::Lost(reason)) => assert!(reason.contains("boom"), "{reason}"),
            other => panic!("the crash was not reported: {other:?}"),
        }
    }

    #[test]
    fn a_run_that_returns_normally_is_not_started_again() {
        let cancel = CancellationToken::new();
        let runs = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let counter = runs.clone();
        run_guarded_with_pause(
            move || {
                let counter = counter.clone();
                async move {
                    counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
            },
            &watch::channel(MouseStatus::Off).0,
            &cancel,
            std::time::Duration::from_millis(10),
        );
        assert_eq!(runs.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    // Final review (ledger minor): the Mac's command receiver was lost when sharing crashed, so
    // Disconnect stopped working until the app restarted.
    #[test]
    fn what_a_run_borrowed_is_put_back_even_after_a_crash() {
        let home = std::sync::Mutex::new(Some(7));
        let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut lent = Lent::take(&home);
            assert_eq!(*lent.item(), Some(7));
            assert_eq!(*home.lock().unwrap(), None, "taken while running");
            panic!("boom");
        }));
        assert!(crashed.is_err());
        assert_eq!(*home.lock().unwrap(), Some(7));
    }

    #[tokio::test]
    async fn a_new_manager_is_off_and_stop_is_harmless() {
        let mut m = MouseManager::new();
        assert_eq!(m.status(), MouseStatus::Off);
        m.stop().await;
        assert_eq!(m.status(), MouseStatus::Off);
    }
}
