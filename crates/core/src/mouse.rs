//! Keeps mouse and keyboard sharing running with the right settings, and reports how it is
//! doing. Sharing is a convenience on top of the display: nothing here ever fails a session.

use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MouseStatus {
    Off,
    Running,
    Failed(String),
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
    running: Option<(MouseRole, CancellationToken, JoinHandle<()>)>,
}

impl Default for MouseManager {
    fn default() -> Self {
        MouseManager { status: watch::channel(MouseStatus::Off).0, running: None }
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

    /// Runs sharing in this role. Does nothing if it is already running in the same one.
    pub async fn start(&mut self, role: MouseRole) {
        if self.running.as_ref().is_some_and(|(current, _, task)| *current == role && !task.is_finished()) {
            return;
        }
        self.stop().await;
        let cancel = CancellationToken::new();
        let (stop, status) = (cancel.clone(), self.status.clone());
        // Lan Mouse's capture and emulation must stay on one thread, so sharing gets its own.
        let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();
        let worker = role.clone();
        std::thread::spawn(move || {
            let guard_status = status.clone();
            let guard_cancel = stop.clone();
            run_guarded(
                move || {
                    let (worker, status, stop) = (worker.clone(), status.clone(), stop.clone());
                    async move {
                        match worker {
                            #[cfg(windows)]
                            MouseRole::Pc(settings) => crate::share::pc::run(settings, status, stop).await,
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

/// How long to wait before starting sharing again after it stopped by itself.
const RESTART_PAUSE: std::time::Duration = std::time::Duration::from_secs(3);

/// Runs sharing on this thread until cancelled, starting it again if it stops by itself or
/// crashes (a crash is reported, not silent). Runs inside a `LocalSet`, which Lan Mouse's code
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
        if let Err(payload) = outcome {
            let what = payload
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown error".to_string());
            status.send_replace(MouseStatus::Failed(format!("mouse sharing crashed ({what}); starting it again")));
        }
        // Stopped by itself (a problem it has already reported) or crashed: try again shortly.
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
                    status.send_replace(MouseStatus::Running);
                    stop.cancel();
                }
            },
            &watch::channel(MouseStatus::Off).0,
            &cancel,
        );
        assert_eq!(*rx.borrow(), MouseStatus::Running);
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
            Some(MouseStatus::Failed(reason)) => assert!(reason.contains("boom"), "{reason}"),
            other => panic!("the crash was not reported: {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_new_manager_is_off_and_stop_is_harmless() {
        let mut m = MouseManager::new();
        assert_eq!(m.status(), MouseStatus::Off);
        m.stop().await;
        assert_eq!(m.status(), MouseStatus::Off);
    }
}
