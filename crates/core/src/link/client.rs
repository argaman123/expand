//! PC side: connects to the Mac.

use super::{client_handshake, Connection, LinkError, CONNECT_TIMEOUT};
use crate::config::Peer;
use std::net::SocketAddr;
use tokio::net::TcpStream;

/// Connects (to whichever of `addrs` answers first) and authenticates. With no peer (or an
/// empty token) it pairs using the Mac's `code` and returns the new peer with `host` set, or
/// fails with `NeedsCode` when there is none. A refused token is `AuthFailed`; it never re-pairs. Also returns the address that answered.
pub async fn connect(
    addrs: &[SocketAddr],
    me_name: &str,
    me_id: &str,
    peer: Option<&Peer>,
    code: Option<&str>,
) -> Result<(Connection, Option<Peer>, SocketAddr), LinkError> {
    let Some((stream, addr)) = first_reachable(addrs, CONNECT_TIMEOUT).await else {
        return Err(LinkError::Unreachable);
    };
    let _ = stream.set_nodelay(true);
    let (stream, new_peer) = client_handshake(stream, me_name, me_id, peer, code).await?;
    let new_peer = new_peer.map(|p| Peer { host: addr.ip().to_string(), ..p });
    Ok((Connection::new(stream), new_peer, addr))
}

/// Whether an address is a direct cable's: link-local (169.254.x.x, fe80::), which is what a
/// Thunderbolt or USB-C network bridge between the two computers gets.
pub fn is_direct(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => v4.is_link_local(),
        std::net::IpAddr::V6(v6) => (v6.segments()[0] & 0xffc0) == 0xfe80,
    }
}

/// How long a direct cable's address is still waited for once another address answered.
pub const DIRECT_GRACE: std::time::Duration = std::time::Duration::from_millis(250);

/// Which of the addresses that `answered` (in the order they did) to use: a direct cable's,
/// else the first, unless a direct cable's address is `direct_pending` (still trying) and the
/// grace for it is not `over`. `None`: wait on.
pub fn choose(answered: &[SocketAddr], direct_pending: bool, over: bool) -> Option<SocketAddr> {
    if let Some(direct) = answered.iter().find(|a| is_direct(a.ip())) {
        return Some(*direct);
    }
    let first = *answered.first()?;
    (!direct_pending || over).then_some(first)
}

/// Tries every address at once and returns a connection that opens, with its address. The Mac
/// may be reachable several ways (home network, Tailscale, a cable): whichever answers first is
/// used, except that a direct cable is preferred when it answers within [`DIRECT_GRACE`] of the
/// first. With no cable address among them, nothing waits.
pub async fn first_reachable(addrs: &[SocketAddr], wait: std::time::Duration) -> Option<(TcpStream, SocketAddr)> {
    use tokio::time::{timeout_at, Instant};
    let mut attempts = tokio::task::JoinSet::new();
    for &addr in addrs {
        attempts.spawn(async move { (addr, TcpStream::connect(addr).await) });
    }
    let deadline = Instant::now() + wait;
    let mut direct_pending = addrs.iter().filter(|a| is_direct(a.ip())).count();
    let mut answered: Vec<(TcpStream, SocketAddr)> = Vec::new();
    let mut grace_ends: Option<Instant> = None;
    let mut all_done = false;
    let found = loop {
        let now = Instant::now();
        let over = all_done || now >= deadline || grace_ends.is_some_and(|g| now >= g);
        let addrs: Vec<SocketAddr> = answered.iter().map(|(_, a)| *a).collect();
        if let Some(chosen) = choose(&addrs, direct_pending > 0, over) {
            let at = answered.iter().position(|(_, a)| *a == chosen)?;
            break Some(answered.swap_remove(at));
        }
        if all_done || now >= deadline {
            break None;
        }
        let until = grace_ends.map_or(deadline, |g| g.min(deadline));
        match timeout_at(until, attempts.join_next()).await {
            Ok(Some(Ok((addr, result)))) => {
                if is_direct(addr.ip()) {
                    direct_pending -= 1;
                }
                if let Ok(stream) = result {
                    answered.push((stream, addr));
                    grace_ends.get_or_insert(Instant::now() + DIRECT_GRACE);
                }
            }
            // An attempt that panicked: its address is unknown, so a cable is waited for until
            // the grace ends.
            Ok(Some(Err(_))) => {}
            Ok(None) => all_done = true,
            // The grace ended (the next round takes what answered), or the time is up.
            Err(_) => {
                if Instant::now() >= deadline {
                    all_done = true;
                }
            }
        }
    };
    attempts.abort_all();
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(ip: &str) -> SocketAddr {
        SocketAddr::new(ip.parse().unwrap(), 47800)
    }

    #[test]
    fn a_cable_is_a_link_local_address() {
        assert!(is_direct("169.254.10.2".parse().unwrap()));
        assert!(is_direct("fe80::1".parse().unwrap()));
        assert!(!is_direct("192.168.1.50".parse().unwrap()));
        assert!(!is_direct("100.64.0.2".parse().unwrap()));
    }

    #[test]
    fn a_cable_that_answers_is_used_even_when_another_answered_first() {
        let lan = at("192.168.1.50");
        let cable = at("169.254.10.2");
        assert_eq!(choose(&[lan, cable], false, false), Some(cable));
    }

    #[test]
    fn the_first_answer_waits_for_a_cable_still_trying_until_the_grace_ends() {
        let lan = at("192.168.1.50");
        assert_eq!(choose(&[lan], true, false), None);
        assert_eq!(choose(&[lan], true, true), Some(lan));
    }

    #[test]
    fn with_no_cable_the_first_answer_is_used_at_once() {
        let (lan, tailscale) = (at("192.168.1.50"), at("100.64.0.2"));
        assert_eq!(choose(&[tailscale, lan], false, false), Some(tailscale));
        assert_eq!(choose(&[], false, true), None);
    }
}
