//! PC side of a session.

use super::{State, StateTx, ViewerEngine};
use crate::config::{Config, Peer, StreamSettings};
use crate::side::Side;
use crate::link::{self, Connection};
use crate::protocol::Message;
use rand::Rng;
use std::net::SocketAddr;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// A viewer that exits this soon after starting never showed a stream.
const VIEWER_MIN_LIFE: Duration = Duration::from_secs(5);
const STOP_ACK_WAIT: Duration = Duration::from_secs(2);
/// How long to let the viewer's pair command finish once Sunshine has accepted the PIN.
const VIEWER_PAIR_WAIT: Duration = Duration::from_secs(5);

fn failed(part: &str, reason: impl Into<String>) -> State {
    State::Failed { part: part.to_string(), reason: reason.into() }
}

/// Waits for a message the caller cares about; `pick` returns `None` to keep waiting.
async fn wait_for<T>(conn: &mut Connection, pick: impl Fn(&Message) -> Option<T>) -> Result<T, State> {
    loop {
        match conn.recv().await {
            Ok(Message::Failed { part, reason }) => return Err(State::Failed { part, reason }),
            Ok(m) => {
                if let Some(v) = pick(&m) {
                    return Ok(v);
                }
            }
            Err(e) => return Err(failed("Connection", e.to_string())),
        }
    }
}

async fn drive(
    conn: &mut Connection,
    host: &str,
    peer: &mut Peer,
    stream: &StreamSettings,
    mac_side: Side,
    viewer: &mut dyn ViewerEngine,
    state: &StateTx,
    cancel: &CancellationToken,
) -> State {
    // First, so the Mac puts the virtual display on the right side when it starts.
    if let Err(e) = conn.send(Message::Arrangement { mac_side }).await {
        return failed("Connection", e.to_string());
    }
    let start = Message::Start { width: stream.width, height: stream.height, fps: stream.fps };
    if let Err(e) = conn.send(start).await {
        return failed("Connection", e.to_string());
    }
    // The Mac says where else it can be reached just before it is ready.
    let reported = std::sync::Mutex::new(None);
    let ready = tokio::select! {
        _ = cancel.cancelled() => return State::Disconnected,
        r = wait_for(conn, |m| match m {
            Message::Addresses { list } => {
                *reported.lock().unwrap() = Some(list.clone());
                None
            }
            Message::Ready => Some(()),
            _ => None,
        }) => r,
    };
    if let Err(state) = ready {
        return state;
    }
    if let Some(list) = reported.into_inner().unwrap() {
        peer.addresses = list.into_iter().filter(|a| *a != peer.host).collect();
    }

    if !peer.sunshine_paired {
        let pin = format!("{:04}", rand::thread_rng().gen_range(0..10000));
        if let Err(e) = conn.send(Message::SunshinePair { pin: pin.clone() }).await {
            return failed("Connection", e.to_string());
        }
        let not_paired = || failed("Pairing", "Moonlight and Sunshine could not be paired");
        let viewer_pairing = viewer.pair(host, &pin);
        let host_answer = wait_for(conn, |m| match m {
            Message::SunshinePairResult { ok } => Some(*ok),
            _ => None,
        });
        tokio::pin!(viewer_pairing, host_answer);

        // Either side can settle it. The viewer reporting success is final (that is also what
        // happens when it was paired already and Sunshine never sees a request). Otherwise the
        // host's answer decides: if Sunshine never took the PIN, the viewer would wait forever.
        let viewer_ok = tokio::select! {
            _ = cancel.cancelled() => return State::Disconnected,
            v = &mut viewer_pairing => v,
            h = &mut host_answer => match h {
                Err(state) => return state,
                Ok(false) => return not_paired(),
                // Sunshine has accepted the PIN, so the pairing itself is done. Moonlight's pair
                // command has been seen to keep running afterwards; give it a moment, then move on.
                Ok(true) => tokio::select! {
                    _ = cancel.cancelled() => return State::Disconnected,
                    v = tokio::time::timeout(VIEWER_PAIR_WAIT, &mut viewer_pairing) => v.unwrap_or(Ok(true)),
                },
            },
        };
        match viewer_ok {
            Ok(true) => peer.sunshine_paired = true,
            _ => return not_paired(),
        }
    }

    if let Err(e) = viewer.start(host, stream).await {
        return failed("Moonlight", e.to_string());
    }
    let viewer_started = tokio::time::Instant::now();
    state.send_replace(State::Connected);

    tokio::select! {
        _ = cancel.cancelled() => State::Disconnected,
        _ = viewer.closed() => {
            if viewer_started.elapsed() < VIEWER_MIN_LIFE {
                // Most likely Moonlight is no longer paired with this Sunshine: pair again next time.
                peer.sunshine_paired = false;
                failed("Moonlight", "the viewer closed right after starting; try again")
            } else {
                State::Disconnected
            }
        }
        r = wait_for(conn, |_| None::<()>) => r.err().unwrap_or(State::Disconnected),
    }
}

/// Runs a session on an established link until it ends, then tears everything down.
/// Returns the peer, with `sunshine_paired` updated.
pub async fn run_connected(
    mut conn: Connection,
    host: &str,
    mut peer: Peer,
    stream: &StreamSettings,
    mac_side: Side,
    viewer: &mut dyn ViewerEngine,
    state: &StateTx,
    cancel: CancellationToken,
) -> Peer {
    state.send_replace(State::Connecting);
    let end = drive(&mut conn, host, &mut peer, stream, mac_side, viewer, state, &cancel).await;

    viewer.stop().await;
    if conn.send(Message::Stop).await.is_ok() {
        let ack = wait_for(&mut conn, |m| matches!(m, Message::Stopped).then_some(()));
        let _ = tokio::time::timeout(STOP_ACK_WAIT, ack).await;
    }
    state.send_replace(end);
    peer
}

/// Connects to the Mac (pairing if needed) and runs a session.
/// Returns the peer to store when anything about it changed.
///
pub async fn run(
    addrs: Vec<SocketAddr>,
    cfg: &Config,
    viewer: &mut dyn ViewerEngine,
    state: &StateTx,
    cancel: CancellationToken,
    save_peer: &(dyn Fn(Peer) + Send + Sync),
) -> Option<Peer> {
    state.send_replace(State::Connecting);
    let connected = tokio::select! {
        _ = cancel.cancelled() => {
            state.send_replace(State::Disconnected);
            return None;
        }
        r = link::client::connect_or_pair(&addrs, &cfg.device_name, &cfg.device_id, cfg.peer.as_ref()) => r,
    };
    let (conn, new_peer, addr) = match connected {
        Ok(ok) => ok,
        Err(e) => {
            state.send_replace(failed("Connection", e.to_string()));
            return None;
        }
    };
    let host = addr.ip().to_string();
    let paired = new_peer.is_some();
    let peer = match (new_peer, &cfg.peer) {
        // Re-paired: keep what the user typed and what the Mac reported before.
        (Some(new), Some(old)) => Peer {
            name: old.name.clone(),
            id: old.id.clone(),
            host: old.host.clone(),
            addresses: old.addresses.clone(),
            ..new
        },
        (Some(new), None) => Peer { name: host.clone(), ..new },
        (None, Some(old)) => old.clone(),
        (None, None) => unreachable!("connecting without a peer always pairs"),
    };
    // Saved straight away: an app closed mid-session must not forget a pairing the Mac keeps.
    if paired {
        save_peer(peer.clone());
    }
    let after = run_connected(conn, &host, peer, &cfg.stream, cfg.mac_side, viewer, state, cancel).await;
    (cfg.peer.as_ref() != Some(&after)).then_some(after)
}
