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
/// includes Tailscale and a direct cable link when they are there; a cable's come first.
pub fn local_addresses() -> Vec<String> {
    let found = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|i| !i.is_loopback())
        .filter_map(|i| match i.ip() {
            std::net::IpAddr::V4(ip) => Some((i.name.clone(), ip)),
            std::net::IpAddr::V6(_) => None,
        })
        .collect();
    order_addresses(found)
}

/// Whether an interface is a direct cable to another computer: macOS's Thunderbolt Bridge
/// (`bridge0`), anything named for Thunderbolt, or a self-assigned (link-local) address.
fn is_cable(interface: &str, ip: std::net::Ipv4Addr) -> bool {
    let name = interface.to_ascii_lowercase();
    ip.is_link_local() || name.starts_with("bridge") || name.contains("thunderbolt")
}

/// The addresses, each once: a direct cable's first, then the rest, each part in order.
fn order_addresses(found: Vec<(String, std::net::Ipv4Addr)>) -> Vec<String> {
    let (mut cable, mut rest): (Vec<_>, Vec<_>) = found.into_iter().partition(|(name, ip)| is_cable(name, *ip));
    cable.sort_by_key(|(_, ip)| *ip);
    rest.sort_by_key(|(_, ip)| *ip);
    let mut out: Vec<String> = Vec::new();
    for (_, ip) in cable.into_iter().chain(rest) {
        let ip = ip.to_string();
        if !out.contains(&ip) {
            out.push(ip);
        }
    }
    out
}

/// Serves one PC until it says Stop, goes silent, or an engine dies.
/// Whatever happens, the engines are stopped before this returns.
///
/// `on_arrangement` is told which side of the PC this Mac sits on.
pub async fn serve(
    conn: Connection,
    engines: &mut dyn HostEngines,
    state: &StateTx,
    on_arrangement: &(dyn Fn(Side) + Send + Sync),
) {
    serve_gated(conn, engines, state, on_arrangement, &|| true).await
}

/// What a PC is told when it asks for the display while another holds the Mac.
pub const IN_USE: &str = "In use";

/// Like [`serve`], but `may_start` is asked when the PC wants the display; when it says no the PC is
/// told [`IN_USE`] and the connection ends without touching the engines or the state, which
/// belong to whoever holds the Mac.
pub async fn serve_gated(
    mut conn: Connection,
    engines: &mut dyn HostEngines,
    state: &StateTx,
    on_arrangement: &(dyn Fn(Side) + Send + Sync),
    may_start: &(dyn Fn() -> bool + Send + Sync),
) {
    let mut running = false;
    let mut started = false;
    loop {
        let event = tokio::select! {
            m = conn.recv() => Event::Message(m.ok()),
            part = engines.exited(), if running => Event::Died(part),
        };
        match event {
            Event::Message(Some(Message::Start { .. })) if !running && !may_start() => {
                let failed = Message::Failed { part: "Mac".to_string(), reason: IN_USE.to_string() };
                let _ = conn.send(failed).await;
                break;
            }
            Event::Message(Some(Message::Start { width, height, fps })) if !running => {
                started = true;
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
    // A connection that never held the display must not undo the one that does.
    if started {
        engines.stop().await;
        state.send_replace(State::Disconnected);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(name: &str, ip: &str) -> (String, std::net::Ipv4Addr) {
        (name.to_string(), ip.parse().unwrap())
    }

    #[test]
    fn a_cable_address_is_listed_first() {
        let found = vec![
            at("en0", "192.168.1.50"),
            at("utun4", "100.64.0.2"),
            at("bridge0", "10.0.7.2"),
            at("en5", "169.254.3.4"),
            at("en0", "192.168.1.50"),
        ];
        assert_eq!(order_addresses(found), ["10.0.7.2", "169.254.3.4", "100.64.0.2", "192.168.1.50"]);
    }

    #[test]
    fn without_a_cable_the_addresses_keep_their_order() {
        let found = vec![at("en0", "192.168.1.50"), at("utun4", "100.64.0.2")];
        assert_eq!(order_addresses(found), ["100.64.0.2", "192.168.1.50"]);
    }
}
