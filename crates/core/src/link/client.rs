//! PC side: connects to the Mac.

use super::{client_handshake, Connection, LinkError, CONNECT_TIMEOUT};
use crate::config::Peer;
use std::net::SocketAddr;
use tokio::net::TcpStream;
use tokio::time::timeout;

/// Connects (to whichever of `addrs` answers first) and authenticates. With no peer (or an
/// empty token) it pairs and returns the new peer with `host` set; `name` and `id` are for the
/// caller to fill in. Also returns the address that answered.
pub async fn connect(
    addrs: &[SocketAddr],
    me_name: &str,
    me_id: &str,
    peer: Option<&Peer>,
) -> Result<(Connection, Option<Peer>, SocketAddr), LinkError> {
    let Some((stream, addr)) = first_reachable(addrs, CONNECT_TIMEOUT).await else {
        return Err(LinkError::Unreachable);
    };
    let _ = stream.set_nodelay(true);
    let (stream, new_peer) = client_handshake(stream, me_name, me_id, peer).await?;
    let new_peer = new_peer.map(|p| Peer { host: addr.ip().to_string(), ..p });
    Ok((Connection::new(stream), new_peer, addr))
}

/// Like [`connect`], but if the Mac no longer trusts this PC's saved token, asks to pair
/// afresh (the person at the Mac still has to allow it). The new peer comes back to be saved.
pub async fn connect_or_pair(
    addrs: &[SocketAddr],
    me_name: &str,
    me_id: &str,
    peer: Option<&Peer>,
) -> Result<(Connection, Option<Peer>, SocketAddr), LinkError> {
    match connect(addrs, me_name, me_id, peer).await {
        Err(LinkError::AuthFailed) => connect(addrs, me_name, me_id, None).await,
        other => other,
    }
}

/// Tries every address at once and returns the first connection that opens, with its address.
/// The Mac may be reachable several ways (home network, Tailscale, a cable): whichever answers
/// first is used, so a direct cable wins when it is plugged in.
pub async fn first_reachable(addrs: &[SocketAddr], wait: std::time::Duration) -> Option<(TcpStream, SocketAddr)> {
    let mut attempts = tokio::task::JoinSet::new();
    for &addr in addrs {
        attempts.spawn(async move { TcpStream::connect(addr).await.map(|s| (s, addr)) });
    }
    let race = async {
        while let Some(done) = attempts.join_next().await {
            if let Ok(Ok(found)) = done {
                return Some(found);
            }
        }
        None
    };
    let found = timeout(wait, race).await.ok().flatten();
    attempts.abort_all();
    found
}
