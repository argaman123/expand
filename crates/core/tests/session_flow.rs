//! Connect/disconnect flows with fake engines, over in-memory pipes with paused time.

use async_trait::async_trait;
use expand_core::config::{Peer, StreamSettings};
use expand_core::side::Side;
use expand_core::link::Connection;
use expand_core::protocol::{write_message, Message};
use expand_core::session::client::run_connected;
use expand_core::session::host::serve;
use expand_core::session::{HostEngines, State, ViewerEngine};
use expand_core::sunshine::EngineError;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{watch, Notify};
use tokio_util::sync::CancellationToken;

type Calls = Arc<Mutex<Vec<String>>>;

fn has(calls: &Calls, what: &str) -> bool {
    calls.lock().unwrap().iter().any(|c| c == what)
}

#[derive(Clone, Copy)]
enum HostStart {
    Ok,
    AlreadyRunning,
    DisplayHelperMissing,
    Hang,
}

struct FakeHost {
    calls: Calls,
    start: HostStart,
    die: Arc<Notify>,
    pin_accepted: bool,
}

#[async_trait]
impl HostEngines for FakeHost {
    async fn start(&mut self, width: u32, height: u32, fps: u32) -> Result<(), EngineError> {
        self.calls.lock().unwrap().push(format!("start({width},{height},{fps})"));
        match self.start {
            HostStart::Ok => Ok(()),
            HostStart::AlreadyRunning => Err(EngineError::AlreadyRunning("Sunshine")),
            HostStart::DisplayHelperMissing => Err(EngineError::NotInstalled("Virtual display")),
            HostStart::Hang => std::future::pending().await,
        }
    }
    async fn submit_pin(&mut self, pin: &str, _client_name: &str) -> Result<bool, EngineError> {
        self.calls.lock().unwrap().push(format!("pin({pin})"));
        Ok(self.pin_accepted)
    }
    async fn stop(&mut self) {
        self.calls.lock().unwrap().push("stop".into());
    }
    async fn exited(&mut self) -> &'static str {
        self.die.notified().await;
        "Sunshine"
    }
}

/// What the viewer says when asked whether it is still paired with the Mac.
#[derive(Clone, Copy)]
enum Says {
    Unknown,
    Paired,
    NotPaired,
    Hangs,
}

struct FakeViewer {
    calls: Calls,
    start_fails: bool,
    close: Arc<Notify>,
    pair_hangs: bool,
    says: Says,
    /// The error it puts up (with `close` notified), if any.
    last_words: Arc<Mutex<Option<String>>>,
}

#[async_trait]
impl ViewerEngine for FakeViewer {
    async fn is_paired(&mut self, host: &str) -> std::io::Result<Option<bool>> {
        self.calls.lock().unwrap().push(format!("is_paired({host})"));
        match self.says {
            Says::Unknown => Ok(None),
            Says::Paired => Ok(Some(true)),
            Says::NotPaired => Ok(Some(false)),
            Says::Hangs => std::future::pending().await,
        }
    }
    async fn pair(&mut self, _host: &str, pin: &str) -> std::io::Result<bool> {
        self.calls.lock().unwrap().push(format!("pair({pin})"));
        if self.pair_hangs {
            std::future::pending::<()>().await;
        }
        Ok(true)
    }
    async fn start(&mut self, host: &str, s: &StreamSettings) -> std::io::Result<()> {
        self.calls.lock().unwrap().push(format!("start({host},{}x{})", s.width, s.height));
        if self.start_fails {
            Err(std::io::Error::new(std::io::ErrorKind::NotFound, "Moonlight.exe missing"))
        } else {
            Ok(())
        }
    }
    async fn closed(&mut self) {
        self.close.notified().await;
    }
    async fn stop(&mut self) {
        self.calls.lock().unwrap().push("stop".into());
    }
    fn last_words(&mut self) -> Option<String> {
        self.last_words.lock().unwrap().take()
    }
}

struct Rig {
    host_calls: Calls,
    viewer_calls: Calls,
    die: Arc<Notify>,
    close: Arc<Notify>,
    /// What the viewer says as it closes (set before notifying `close`).
    last_words: Arc<Mutex<Option<String>>>,
    cancel: CancellationToken,
    state: watch::Receiver<State>,
    client: tokio::task::JoinHandle<Peer>,
    host: tokio::task::JoinHandle<()>,
}

fn peer(sunshine_paired: bool) -> Peer {
    Peer {
        name: "Mac".into(),
        id: "mac".into(),
        host: "192.168.1.50".into(),
        token: "a".repeat(64),
        sunshine_paired,
        addresses: Vec::new(),
        placement: Default::default(),
        stream: Default::default(),
        allowed: true,
    }
}

fn rig(host_start: HostStart, viewer_start_fails: bool, sunshine_paired: bool) -> Rig {
    rig_with(host_start, viewer_start_fails, sunshine_paired, true, false)
}

/// `pin_accepted`: whether Sunshine takes the PIN. `pair_hangs`: Moonlight's pair command never exits.
fn rig_with(
    host_start: HostStart,
    viewer_start_fails: bool,
    sunshine_paired: bool,
    pin_accepted: bool,
    pair_hangs: bool,
) -> Rig {
    rig_full(host_start, viewer_start_fails, sunshine_paired, pin_accepted, pair_hangs, Says::Unknown)
}

/// The PC's records say paired; `says` is what the viewer answers when asked.
fn rig_says(says: Says, pin_accepted: bool) -> Rig {
    // A refused PIN leaves Moonlight's pair command waiting, as the real one does.
    rig_full(HostStart::Ok, false, true, pin_accepted, !pin_accepted, says)
}

fn rig_full(
    host_start: HostStart,
    viewer_start_fails: bool,
    sunshine_paired: bool,
    pin_accepted: bool,
    pair_hangs: bool,
    says: Says,
) -> Rig {
    let (a, b) = tokio::io::duplex(8192);
    let (host_calls, viewer_calls): (Calls, Calls) = Default::default();
    let (die, close) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    let cancel = CancellationToken::new();
    let (state_tx, state) = watch::channel(State::Disconnected);

    let mut fake_host = FakeHost { calls: host_calls.clone(), start: host_start, die: die.clone(), pin_accepted };
    let told = host_calls.clone();
    let host = tokio::spawn(async move {
        let (host_state, _keep) = watch::channel(State::Disconnected);
        let on_arrangement = move |mac_side: Side| told.lock().unwrap().push(format!("arrangement({mac_side:?})"));
        serve(Connection::new(b), &mut fake_host, &host_state, &on_arrangement).await;
    });

    let mut fake_viewer = FakeViewer {
        calls: viewer_calls.clone(),
        start_fails: viewer_start_fails,
        close: close.clone(),
        pair_hangs,
        says,
        last_words: Default::default(),
    };
    let last_words = fake_viewer.last_words.clone();
    let token = cancel.clone();
    let client = tokio::spawn(async move {
        run_connected(
            Connection::new(a),
            "192.168.1.50",
            peer(sunshine_paired),
            &StreamSettings::default(),
            Side::Right,
            &mut fake_viewer,
            &state_tx,
            token,
        )
        .await
    });

    Rig { host_calls, viewer_calls, die, close, last_words, cancel, state, client, host }
}

async fn wait_for(state: &mut watch::Receiver<State>, want: impl Fn(&State) -> bool) -> State {
    tokio::time::timeout(Duration::from_secs(60), state.wait_for(|s| want(s)))
        .await
        .expect("state never reached")
        .unwrap()
        .clone()
}

fn failed_part(s: &State) -> Option<(&str, &str)> {
    match s {
        State::Failed { part, reason } => Some((part, reason)),
        _ => None,
    }
}

#[tokio::test(start_paused = true)]
async fn happy_path() {
    let mut r = rig(HostStart::Ok, false, true);
    wait_for(&mut r.state, |s| *s == State::Connected).await;
    assert!(has(&r.host_calls, "start(2560,1440,60)"));
    assert!(has(&r.viewer_calls, "start(192.168.1.50,2560x1440)"));
    assert!(!r.viewer_calls.lock().unwrap().iter().any(|c| c.starts_with("pair")));

    r.cancel.cancel();
    r.client.await.unwrap();
    r.host.await.unwrap();
    assert_eq!(*r.state.borrow(), State::Disconnected);
    assert!(has(&r.host_calls, "stop"));
    assert!(has(&r.viewer_calls, "stop"));
}

#[tokio::test(start_paused = true)]
async fn first_run_pairs_sunshine_then_streams() {
    let mut r = rig(HostStart::Ok, false, false);
    wait_for(&mut r.state, |s| *s == State::Connected).await;
    let viewer_pin = r.viewer_calls.lock().unwrap().iter().find(|c| c.starts_with("pair(")).cloned().unwrap();
    let pin = viewer_pin.trim_start_matches("pair(").trim_end_matches(')').to_string();
    assert_eq!(pin.len(), 4);
    assert!(pin.chars().all(|c| c.is_ascii_digit()));
    assert!(has(&r.host_calls, &format!("pin({pin})")));
    // Not paired by our own records: pairing anyway, so there is nothing to ask the viewer.
    assert!(!r.viewer_calls.lock().unwrap().iter().any(|c| c.starts_with("is_paired")));

    r.cancel.cancel();
    assert!(r.client.await.unwrap().sunshine_paired);
}

#[tokio::test(start_paused = true)]
async fn host_start_failure_is_reported_and_cleaned_up() {
    for (start, part, reason) in [
        (HostStart::AlreadyRunning, "Sunshine", "already running"),
        (HostStart::DisplayHelperMissing, "Virtual display", "not found"),
    ] {
        let mut r = rig(start, false, true);
        let s = wait_for(&mut r.state, |s| matches!(s, State::Failed { .. })).await;
        let (got_part, got_reason) = failed_part(&s).unwrap();
        assert_eq!(got_part, part);
        assert!(got_reason.contains(reason), "{got_reason}");
        r.client.await.unwrap();
        r.host.await.unwrap();
        assert!(has(&r.host_calls, "stop"));
        assert!(!r.viewer_calls.lock().unwrap().iter().any(|c| c.starts_with("start")));
    }
}

#[tokio::test(start_paused = true)]
async fn viewer_start_failure_stops_the_host() {
    let mut r = rig(HostStart::Ok, true, true);
    let s = wait_for(&mut r.state, |s| matches!(s, State::Failed { .. })).await;
    assert_eq!(failed_part(&s).unwrap().0, "Moonlight");
    r.client.await.unwrap();
    r.host.await.unwrap();
    assert!(has(&r.host_calls, "stop"));
}

#[tokio::test(start_paused = true)]
async fn closing_viewer_window_disconnects() {
    let mut r = rig(HostStart::Ok, false, true);
    wait_for(&mut r.state, |s| *s == State::Connected).await;
    tokio::time::sleep(Duration::from_secs(10)).await;
    r.close.notify_one();
    r.client.await.unwrap();
    r.host.await.unwrap();
    assert_eq!(*r.state.borrow(), State::Disconnected);
    assert!(has(&r.host_calls, "stop"));
}

#[tokio::test(start_paused = true)]
async fn viewer_closing_immediately_is_a_failure_and_forces_repairing() {
    let mut r = rig(HostStart::Ok, false, true);
    r.close.notify_one();
    let s = wait_for(&mut r.state, |s| matches!(s, State::Failed { .. })).await;
    assert_eq!(failed_part(&s).unwrap().0, "Moonlight");
    assert!(!r.client.await.unwrap().sunshine_paired);
    r.host.await.unwrap();
    assert!(has(&r.host_calls, "stop"));
}

/// Moonlight putting up an error ends the session with it, whenever it comes; Moonlight goes
/// with its message, and the pairing is kept.
#[tokio::test(start_paused = true)]
async fn an_error_the_viewer_puts_up_is_the_failure() {
    for after in [Duration::from_secs(1), Duration::from_secs(30)] {
        let mut r = rig(HostStart::Ok, false, true);
        wait_for(&mut r.state, |s| *s == State::Connected).await;
        tokio::time::sleep(after).await;
        *r.last_words.lock().unwrap() = Some("Failed to connect to the Mac".into());
        r.close.notify_one();
        let s = wait_for(&mut r.state, |s| matches!(s, State::Failed { .. })).await;
        assert_eq!(failed_part(&s), Some(("Moonlight", "Failed to connect to the Mac")));
        assert!(r.client.await.unwrap().sunshine_paired);
        r.host.await.unwrap();
        assert!(has(&r.viewer_calls, "stop"));
        assert!(has(&r.host_calls, "stop"));
    }
}

/// An error that says Moonlight is not paired: it pairs again next time.
#[tokio::test(start_paused = true)]
async fn an_error_saying_not_paired_pairs_again_next_time() {
    let mut r = rig(HostStart::Ok, false, true);
    wait_for(&mut r.state, |s| *s == State::Connected).await;
    *r.last_words.lock().unwrap() = Some("Computer Mac has not been paired. Please open Moonlight to pair before streaming.".into());
    r.close.notify_one();
    let s = wait_for(&mut r.state, |s| matches!(s, State::Failed { .. })).await;
    assert_eq!(failed_part(&s).unwrap().0, "Moonlight");
    assert!(!r.client.await.unwrap().sunshine_paired);
}

#[tokio::test(start_paused = true)]
async fn engine_death_notifies_pc() {
    let mut r = rig(HostStart::Ok, false, true);
    wait_for(&mut r.state, |s| *s == State::Connected).await;
    r.die.notify_one();
    let s = wait_for(&mut r.state, |s| matches!(s, State::Failed { .. })).await;
    assert_eq!(failed_part(&s).unwrap().0, "Sunshine");
    r.client.await.unwrap();
    assert!(has(&r.viewer_calls, "stop"));
}

#[tokio::test(start_paused = true)]
async fn cancel_during_connecting_tears_down() {
    let mut r = rig(HostStart::Hang, false, true);
    wait_for(&mut r.state, |s| *s == State::Connecting).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    r.cancel.cancel();
    r.client.await.unwrap();
    r.host.await.unwrap();
    assert_eq!(*r.state.borrow(), State::Disconnected);
    assert!(has(&r.host_calls, "stop"));
    assert!(has(&r.viewer_calls, "stop"));
}

#[tokio::test(start_paused = true)]
async fn pc_vanishes_host_tears_down_at_10s() {
    // A raw client that asks to start and then goes silent, with no heartbeats.
    let (mut raw_client, b) = tokio::io::duplex(8192);
    let calls: Calls = Default::default();
    let mut fake_host = FakeHost { calls: calls.clone(), start: HostStart::Ok, die: Arc::new(Notify::new()), pin_accepted: true };
    write_message(&mut raw_client, &Message::Start { width: 2560, height: 1440, fps: 60 }).await.unwrap();

    let started = tokio::time::Instant::now();
    let (host_state, _keep) = watch::channel(State::Disconnected);
    serve(Connection::new(b), &mut fake_host, &host_state, &|_| {}).await;
    let waited = started.elapsed();

    assert!(has(&calls, "start(2560,1440,60)"));
    assert!(has(&calls, "stop"));
    assert!(waited >= Duration::from_secs(9) && waited <= Duration::from_secs(12), "waited {waited:?}");
    drop(raw_client);
}

#[tokio::test(start_paused = true)]
async fn link_loss_while_connected_is_a_failure() {
    let mut r = rig(HostStart::Ok, false, true);
    wait_for(&mut r.state, |s| *s == State::Connected).await;
    r.host.abort();
    let s = wait_for(&mut r.state, |s| matches!(s, State::Failed { .. })).await;
    assert_eq!(failed_part(&s).unwrap().0, "Connection");
    r.client.await.unwrap();
    assert!(has(&r.viewer_calls, "stop"));
}

#[tokio::test(start_paused = true)]
async fn pairing_refused_by_host_fails_without_waiting_for_the_viewer() {
    let mut r = rig_with(HostStart::Ok, false, false, false, true);
    let s = wait_for(&mut r.state, |s| matches!(s, State::Failed { .. })).await;
    assert_eq!(failed_part(&s).unwrap().0, "Pairing");
    assert!(!r.client.await.unwrap().sunshine_paired);
    r.host.await.unwrap();
    assert!(has(&r.host_calls, "stop"));
    assert!(!r.viewer_calls.lock().unwrap().iter().any(|c| c.starts_with("start")));
}

// Seen with the real Moonlight on 2026-11-11: pairing completed but the pair command kept running.
#[tokio::test(start_paused = true)]
async fn pairing_accepted_by_host_proceeds_even_if_the_pair_command_never_exits() {
    let mut r = rig_with(HostStart::Ok, false, false, true, true);
    wait_for(&mut r.state, |s| *s == State::Connected).await;
    assert!(r.viewer_calls.lock().unwrap().iter().any(|c| c.starts_with("start(")));
    r.cancel.cancel();
    assert!(r.client.await.unwrap().sunshine_paired);
}

// Moonlight already paired with this Sunshine: its pair command succeeds at once and Sunshine
// never sees a request to give a PIN to.
#[tokio::test(start_paused = true)]
async fn viewer_already_paired_counts_as_paired() {
    let mut r = rig_with(HostStart::Ok, false, false, false, false);
    wait_for(&mut r.state, |s| *s == State::Connected).await;
    r.cancel.cancel();
    assert!(r.client.await.unwrap().sunshine_paired);
}

// Review finding: the Mac must notice a cancel during first-time pairing straight away,
// not after the ten seconds of PIN retries.
#[tokio::test(start_paused = true)]
async fn cancel_during_pairing_stops_the_host_promptly() {
    let mut r = rig_with(HostStart::Ok, false, false, false, true);
    wait_for(&mut r.state, |s| *s == State::Connecting).await;
    tokio::time::sleep(Duration::from_secs(2)).await; // host is now retrying the PIN
    let cancelled_at = tokio::time::Instant::now();
    r.cancel.cancel();
    r.client.await.unwrap();
    r.host.await.unwrap();
    assert!(has(&r.host_calls, "stop"));
    assert!(cancelled_at.elapsed() < Duration::from_secs(3), "host took {:?}", cancelled_at.elapsed());
}

// 2026-11-19: the Mac reports every address it has, so the PC can also reach it over Tailscale
// or a cable next time.
#[tokio::test(start_paused = true)]
async fn the_mac_reports_its_other_addresses() {
    let mut r = rig(HostStart::Ok, false, true);
    wait_for(&mut r.state, |s| *s == State::Connected).await;
    r.cancel.cancel();
    let peer = r.client.await.unwrap();
    let expected: Vec<String> =
        expand_core::session::host::local_addresses().into_iter().filter(|a| a != "192.168.1.50").collect();
    assert_eq!(peer.addresses, expected);
}

// The Mac places the virtual display when it starts, so it must know its side before that.
#[tokio::test(start_paused = true)]
async fn the_mac_learns_which_side_it_sits_on_before_starting() {
    let mut r = rig(HostStart::Ok, false, true);
    wait_for(&mut r.state, |s| *s == State::Connected).await;
    let calls = r.host_calls.lock().unwrap().clone();
    let told = calls.iter().position(|c| c == "arrangement(Right)").expect("never told");
    let started = calls.iter().position(|c| c.starts_with("start(")).expect("never started");
    assert!(told < started, "{calls:?}");
    r.cancel.cancel();
}

// 2026-11-18: a new pairing was only saved on the PC when the session ended, so an app that was
// closed hard mid-session forgot it while the Mac remembered: "no longer trusts this one".
#[tokio::test]
async fn a_new_pairing_is_saved_as_soon_as_it_is_made() {
    use expand_core::config::Config;
    use expand_core::link::{server::Server, PairGate};

    struct Allow;
    impl PairGate for Allow {
        fn code(&self) -> Option<String> {
            // The code the PC below pairs with.
            Some("123456".to_string())
        }

        fn conclude(&self, _code: &str, proved: bool) -> bool {
            proved
        }
    }

    let mut server = Server::bind("127.0.0.1:0".parse().unwrap()).await.unwrap();
    let addr = server.local_addr();
    tokio::spawn(async move {
        let (conn, _, _) = server.accept(|_: &str| None, Arc::new(Allow), ("MacBook", "macid")).await.unwrap();
        let calls: Calls = Default::default();
        let mut host = FakeHost { calls, start: HostStart::Hang, die: Arc::new(Notify::new()), pin_accepted: true };
        let (state, _) = watch::channel(State::Disconnected);
        serve(conn, &mut host, &state, &|_: Side| {}).await;
    });

    let saved: Arc<Mutex<Option<Peer>>> = Default::default();
    let save = { let saved = saved.clone(); move |p: Peer| *saved.lock().unwrap() = Some(p) };
    let mut viewer = FakeViewer {
        calls: Default::default(),
        start_fails: false,
        close: Arc::new(Notify::new()),
        pair_hangs: false,
        says: Says::Unknown,
        last_words: Default::default(),
    };
    let (state, _) = watch::channel(State::Disconnected);
    let cancel = CancellationToken::new();
    let cfg = Config::default_new();
    let session = {
        let cancel = cancel.clone();
        async move { expand_core::session::client::run(vec![addr], &cfg, &mut viewer, &state, cancel, &save, Some("123456")).await }
    };
    let watcher = async {
        // The Mac never gets past starting, so the session is still running here.
        for _ in 0..50 {
            if saved.lock().unwrap().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let paired = saved.lock().unwrap().clone();
        cancel.cancel();
        paired
    };
    let (_, paired) = tokio::join!(session, watcher);
    let paired = paired.expect("the pairing was not saved while the session was running");
    assert_eq!(paired.token.len(), 64);
    assert_eq!(paired.host, "127.0.0.1");
}

#[tokio::test(start_paused = true)]
async fn a_start_from_a_pc_that_may_not_start_is_told_the_mac_is_in_use() {
    use expand_core::session::host::serve_gated;
    let (a, b) = tokio::io::duplex(8192);
    let calls: Calls = Default::default();
    let host = {
        let calls = calls.clone();
        tokio::spawn(async move {
            let mut fake = FakeHost { calls, start: HostStart::Ok, die: Arc::new(Notify::new()), pin_accepted: true };
            let (state, _keep) = watch::channel(State::Connected);
            serve_gated(Connection::new(b), &mut fake, &state, &|_: Side| {}, &|| false).await;
            // Someone else's display is not this connection's to end.
            assert_eq!(*state.borrow(), State::Connected);
        })
    };
    let mut pc = Connection::new(a);
    pc.send(Message::Start { width: 1920, height: 1080, fps: 60 }).await.unwrap();
    assert_eq!(pc.recv().await.unwrap(), Message::Failed { part: "Mac".into(), reason: "In use".into() });
    host.await.unwrap();
    assert!(!calls.lock().unwrap().iter().any(|c| c.starts_with("start(") || c == "stop"), "{:?}", calls.lock().unwrap());
}

fn position(calls: &Calls, prefix: &str) -> Option<usize> {
    calls.lock().unwrap().iter().position(|c| c.starts_with(prefix))
}

// Live check, task 9: the PC's records said paired, Moonlight no longer was, so it opened its own
// "has not been paired" dialog and the display showed as running. The viewer is asked first now.
#[tokio::test(start_paused = true)]
async fn a_viewer_that_is_no_longer_paired_pairs_again_before_streaming() {
    let mut r = rig_says(Says::NotPaired, true);
    wait_for(&mut r.state, |s| *s == State::Connected).await;
    let asked = position(&r.viewer_calls, "is_paired(192.168.1.50)").expect("the viewer was never asked");
    let paired = position(&r.viewer_calls, "pair(").expect("never paired");
    let started = position(&r.viewer_calls, "start(").expect("never started");
    assert!(asked < paired && paired < started, "{:?}", r.viewer_calls.lock().unwrap());
    let viewer_pin = r.viewer_calls.lock().unwrap()[paired].clone();
    let pin = viewer_pin.trim_start_matches("pair(").trim_end_matches(')').to_string();
    assert!(has(&r.host_calls, &format!("pin({pin})")), "the PIN went to the Mac");
    r.cancel.cancel();
    assert!(r.client.await.unwrap().sunshine_paired);
}

#[tokio::test(start_paused = true)]
async fn a_viewer_that_says_it_is_paired_streams_without_pairing() {
    let mut r = rig_says(Says::Paired, true);
    wait_for(&mut r.state, |s| *s == State::Connected).await;
    assert!(has(&r.viewer_calls, "is_paired(192.168.1.50)"));
    assert!(position(&r.viewer_calls, "pair(").is_none());
    assert!(!r.host_calls.lock().unwrap().iter().any(|c| c.starts_with("pin(")));
    r.cancel.cancel();
    assert!(r.client.await.unwrap().sunshine_paired);
}

// When the viewer cannot tell, the session goes on as before: straight to streaming.
#[tokio::test(start_paused = true)]
async fn a_viewer_that_cannot_tell_streams_as_before() {
    let mut r = rig_says(Says::Unknown, true);
    wait_for(&mut r.state, |s| *s == State::Connected).await;
    assert!(has(&r.viewer_calls, "is_paired(192.168.1.50)"));
    assert!(position(&r.viewer_calls, "pair(").is_none());
    r.cancel.cancel();
    assert!(r.client.await.unwrap().sunshine_paired);
}

#[tokio::test(start_paused = true)]
async fn pairing_again_refused_by_the_mac_is_remembered_as_not_paired() {
    let mut r = rig_says(Says::NotPaired, false);
    let s = wait_for(&mut r.state, |s| matches!(s, State::Failed { .. })).await;
    assert_eq!(failed_part(&s).unwrap().0, "Pairing");
    assert!(!r.client.await.unwrap().sunshine_paired);
    assert!(position(&r.viewer_calls, "start(").is_none());
}

#[tokio::test(start_paused = true)]
async fn cancel_while_asking_the_viewer_tears_down() {
    let mut r = rig_says(Says::Hangs, true);
    wait_for(&mut r.state, |s| *s == State::Connecting).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(has(&r.viewer_calls, "is_paired(192.168.1.50)"));
    r.cancel.cancel();
    r.client.await.unwrap();
    r.host.await.unwrap();
    assert_eq!(*r.state.borrow(), State::Disconnected);
    assert!(has(&r.host_calls, "stop"));
}
