//! Mac side: listens for the PC.

use super::{server_handshake, Connection, LinkError, TrustPrompt};
use crate::config::Peer;
use std::net::SocketAddr;
use tokio::net::TcpListener;

pub struct Server {
    listener: TcpListener,
}

impl Server {
    pub async fn bind(addr: SocketAddr) -> Result<Server, LinkError> {
        Ok(Server { listener: TcpListener::bind(addr).await.map_err(LinkError::Io)? })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.listener.local_addr().expect("a bound listener has an address")
    }

    /// Accepts one client. A known peer is challenged; an unknown one must pair and be approved,
    /// in which case the new peer is returned for the caller to store.
    ///
    /// `known` is asked for the trusted peer only once a client has connected, so forgetting a
    /// peer takes effect for the very next connection even if this call was already waiting.
    pub async fn accept(
        &self,
        known: impl Fn() -> Option<Peer>,
        prompt: &dyn TrustPrompt,
    ) -> Result<(Connection, Option<Peer>), LinkError> {
        let (stream, remote) = self.listener.accept().await.map_err(LinkError::Io)?;
        let _ = stream.set_nodelay(true);
        let known = known();
        let (stream, new_peer) = server_handshake(stream, known.as_ref(), prompt).await?;
        let new_peer = new_peer.map(|p| Peer { host: remote.ip().to_string(), ..p });
        Ok((Connection::new(stream), new_peer))
    }
}
