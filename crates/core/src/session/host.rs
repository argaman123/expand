//! Mac side of a session.

use super::{HostEngines, State, StateTx};
use crate::side::Side;
use crate::link::Connection;
use crate::protocol::Message;
use std::time::Duration;

/// The viewer has to ask Sunshine to pair before the PIN can be submitted, so keep trying.
const PIN_ATTEMPTS: u32 = 20;
const PIN_RETRY: Duration = Duration::from_millis(500);
const CLIENT_NAME: &str = "Sidescreen PC";

enum Event {
    Message(Option<Message>),
    Died(&'static str),
}

async fn submit_pin_with_retries(engines: &mut dyn HostEngines, pin: &str) -> bool {
    for _ in 0..PIN_ATTEMPTS {
        if let Ok(true) = engines.submit_pin(pin, CLIENT_NAME).await {
            return true;
        }
        tokio::time::sleep(PIN_RETRY).await;
    }
    false
}

/// This computer's addresses other computers might reach it at: IPv4, not loopback. That
/// includes Tailscale and a direct cable link when they are there.
pub fn local_addresses() -> Vec<String> {
    let mut out: Vec<String> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|i| !i.is_loopback())
        .filter_map(|i| match i.ip() {
            std::net::IpAddr::V4(ip) => Some(ip.to_string()),
            std::net::IpAddr::V6(_) => None,
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Serves one PC until it says Stop, goes silent, or an engine dies.
/// Whatever happens, the engines are stopped before this returns.
///
/// `on_arrangement` is told which side of the PC this Mac sits on.
pub async fn serve(
    mut conn: Connection,
    engines: &mut dyn HostEngines,
    state: &StateTx,
    on_arrangement: &(dyn Fn(Side) + Send + Sync),
) {
    let mut running = false;
    loop {
        let event = tokio::select! {
            m = conn.recv() => Event::Message(m.ok()),
            part = engines.exited(), if running => Event::Died(part),
        };
        match event {
            Event::Message(Some(Message::Start { width, height, fps })) if !running => {
                state.send_replace(State::Connecting);
                // Anything arriving while starting is a Stop or a lost link: abandon the start.
                let outcome = tokio::select! {
                    r = engines.start(width, height, fps) => Some(r),
                    _ = conn.recv() => None,
                };
                match outcome {
                    Some(Ok(())) => {
                        running = true;
                        state.send_replace(State::Connected);
                        let addresses = Message::Addresses { list: local_addresses() };
                        if conn.send(addresses).await.is_err() || conn.send(Message::Ready).await.is_err() {
                            break;
                        }
                    }
                    Some(Err(e)) => {
                        let failed = Message::Failed { part: e.part().to_string(), reason: e.to_string() };
                        let _ = conn.send(failed).await;
                        break;
                    }
                    None => break,
                }
            }
            Event::Message(Some(Message::SunshinePair { pin })) if running => {
                // Anything arriving while pairing is a Stop or a lost link: give up at once.
                let ok = tokio::select! {
                    ok = submit_pin_with_retries(engines, &pin) => ok,
                    _ = conn.recv() => break,
                };
                if conn.send(Message::SunshinePairResult { ok }).await.is_err() {
                    break;
                }
            }
            Event::Message(Some(Message::Arrangement { mac_side })) => on_arrangement(mac_side),
            Event::Message(Some(Message::Stop)) => {
                engines.stop().await;
                let _ = conn.send(Message::Stopped).await;
                break;
            }
            Event::Message(Some(_)) => {}
            Event::Message(None) => break,
            Event::Died(part) => {
                let failed = Message::Failed { part: part.to_string(), reason: "stopped unexpectedly".to_string() };
                let _ = conn.send(failed).await;
                break;
            }
        }
    }
    engines.stop().await;
    state.send_replace(State::Disconnected);
}
