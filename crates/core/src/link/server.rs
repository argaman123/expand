//! Mac side: listens for the PC.

use super::{server_handshake_by_id, Connection, LinkError, PairGate};
use crate::config::Peer;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::BufReader;
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinSet;

/// Handshakes that may run at once, from anywhere and from one address. A caller that says
/// nothing holds only its own place, and only for [`HANDSHAKE_DEADLINE`]: it never stands in
/// the way of a PC that answers.
pub const MAX_HANDSHAKES: usize = 32;
pub const MAX_HANDSHAKES_PER_ADDRESS: usize = 4;
/// The longest a whole handshake (authentication or pairing) may take.
pub const HANDSHAKE_DEADLINE: Duration = Duration::from_secs(10);

type Handshake = Result<(BufReader<TcpStream>, Option<Peer>, String), LinkError>;

/// The handshakes running now, by address.
#[derive(Default)]
struct Slots {
    total: usize,
    by_address: HashMap<IpAddr, usize>,
}

/// One running handshake's place; given back however the handshake ends (even aborted).
struct Slot {
    slots: Arc<Mutex<Slots>>,
    address: IpAddr,
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut slots = self.slots.lock().unwrap();
        slots.total -= 1;
        if let Some(n) = slots.by_address.get_mut(&self.address) {
            *n -= 1;
            if *n == 0 {
                slots.by_address.remove(&self.address);
            }
        }
    }
}

pub struct Server {
    listener: TcpListener,
    running: JoinSet<(SocketAddr, Handshake)>,
    slots: Arc<Mutex<Slots>>,
    max_total: usize,
    max_per_address: usize,
    deadline: Duration,
}

impl Server {
    pub async fn bind(addr: SocketAddr) -> Result<Server, LinkError> {
        Ok(Server {
            listener: TcpListener::bind(addr).await.map_err(LinkError::Io)?,
            running: JoinSet::new(),
            slots: Arc::default(),
            max_total: MAX_HANDSHAKES,
            max_per_address: MAX_HANDSHAKES_PER_ADDRESS,
            deadline: HANDSHAKE_DEADLINE,
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.listener.local_addr().expect("a bound listener has an address")
    }

    /// The next client that has finished its handshake. A known peer is challenged; an unknown
    /// one must pair with the Mac's code, in which case the new peer is returned for the caller
    /// to store. A handshake that failed is returned as its error.
    ///
    /// Also returns the id the PC called itself by, which is the one that proved itself (or, when
    /// pairing, the new peer's).
    ///
    /// Each handshake runs in its own task (see [`MAX_HANDSHAKES`]), so a slow or silent caller
    /// holds up nobody. Handshakes started during an earlier call that was given up (dropped)
    /// go on, and are returned by a later call.
    ///
    /// `known` looks up the PC by its id and returns `None` for an unknown or blocked PC. It is
    /// asked only once a client has said who it is, so forgetting a peer takes effect for the
    /// very next connection even if this call was already waiting.
    pub async fn accept(
        &mut self,
        known: impl Fn(&str) -> Option<Peer> + Send + Sync + 'static,
        gate: Arc<dyn PairGate>,
        me: (&str, &str),
    ) -> Result<(Connection, Option<Peer>, String), LinkError> {
        self.accept_from(known, gate, me).await.map(|(conn, new_peer, pc_id, _)| (conn, new_peer, pc_id))
    }

    /// [`Server::accept`], also returning the address the PC connected from, so the Mac can keep
    /// the PC's address up to date.
    pub async fn accept_from(
        &mut self,
        known: impl Fn(&str) -> Option<Peer> + Send + Sync + 'static,
        gate: Arc<dyn PairGate>,
        me: (&str, &str),
    ) -> Result<(Connection, Option<Peer>, String, SocketAddr), LinkError> {
        let known = Arc::new(known);
        let me = (me.0.to_string(), me.1.to_string());
        loop {
            tokio::select! {
                accepted = self.listener.accept() => {
                    let (stream, remote) = accepted.map_err(LinkError::Io)?;
                    // Over the limit: closed at once.
                    let Some(slot) = self.take_slot(remote.ip()) else { continue };
                    let _ = stream.set_nodelay(true);
                    let (known, gate, me, deadline) = (known.clone(), gate.clone(), me.clone(), self.deadline);
                    self.running.spawn(async move {
                        let _slot = slot;
                        let lookup = move |id: &str| known(id);
                        let handshake = server_handshake_by_id(stream, lookup, &*gate, (&me.0, &me.1));
                        // Running out of time drops the handshake, which ends a pairing exchange
                        // as a miss.
                        (remote, tokio::time::timeout(deadline, handshake).await.unwrap_or(Err(LinkError::Closed)))
                    });
                }
                Some(done) = self.running.join_next(), if !self.running.is_empty() => {
                    // A handshake that panicked has nothing to give.
                    let Ok((remote, handshake)) = done else { continue };
                    let (stream, new_peer, pc_id) = handshake?;
                    let new_peer = new_peer.map(|p| Peer { host: remote.ip().to_string(), ..p });
                    return Ok((Connection::new(stream), new_peer, pc_id, remote));
                }
            }
        }
    }

    fn take_slot(&self, address: IpAddr) -> Option<Slot> {
        let mut slots = self.slots.lock().unwrap();
        let from_there = slots.by_address.get(&address).copied().unwrap_or(0);
        if slots.total >= self.max_total || from_there >= self.max_per_address {
            return None;
        }
        slots.total += 1;
        *slots.by_address.entry(address).or_default() += 1;
        Some(Slot { slots: self.slots.clone(), address })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::link::client;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::AsyncReadExt;

    /// Pairing mode with a code, counting the exchanges that ended without it.
    #[derive(Default)]
    struct Counting {
        misses: AtomicUsize,
    }

    impl PairGate for Counting {
        fn code(&self) -> Option<String> {
            Some("123456".to_string())
        }

        fn conclude(&self, _code: &str, proved: bool) -> bool {
            if !proved {
                self.misses.fetch_add(1, Ordering::SeqCst);
            }
            proved
        }
    }

    const TOKEN: &str = "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1";

    fn known_pc() -> Peer {
        Peer {
            name: "pc".into(),
            id: "pcid".into(),
            host: String::new(),
            token: TOKEN.into(),
            sunshine_paired: false,
            addresses: Vec::new(),
            placement: Default::default(),
            stream: Default::default(),
            allowed: true,
        }
    }

    /// A Mac answering on loopback with `server` and `gate`: every handshake's outcome goes to the
    /// returned channel (`true` for a PC let in).
    fn serve(mut server: Server, gate: Arc<dyn PairGate>) -> tokio::sync::mpsc::UnboundedReceiver<bool> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            loop {
                let known = |id: &str| (id == "pcid").then(known_pc);
                let r = server.accept(known, gate.clone(), ("MacBook", "macid")).await;
                if tx.send(r.is_ok()).is_err() {
                    break;
                }
            }
        });
        rx
    }

    /// True when the other end closes `stream` within `wait`.
    async fn closed_within(stream: &mut TcpStream, wait: Duration) -> bool {
        let mut byte = [0u8; 1];
        // The challenge never comes for a caller that never says hello; only the close does.
        matches!(tokio::time::timeout(wait, stream.read(&mut byte)).await, Ok(Ok(0)) | Ok(Err(_)))
    }

    #[tokio::test]
    async fn a_silent_caller_does_not_hold_up_a_known_pc() {
        let server = Server::bind("127.0.0.1:0".parse().unwrap()).await.unwrap();
        let addr = server.local_addr();
        let mut outcomes = serve(server, Arc::new(Counting::default()));
        let _silent = TcpStream::connect(addr).await.unwrap();
        let known = async {
            let r = client::connect(&[addr], "pc", "pcid", Some(&known_pc()), None).await;
            (r.is_ok(), outcomes.recv().await)
        };
        let got = tokio::time::timeout(Duration::from_secs(2), known).await.expect("not held up");
        assert_eq!(got, (true, Some(true)));
    }

    #[tokio::test]
    async fn a_silent_caller_is_dropped_at_the_deadline() {
        let mut server = Server::bind("127.0.0.1:0".parse().unwrap()).await.unwrap();
        server.deadline = Duration::from_millis(200);
        let addr = server.local_addr();
        let mut outcomes = serve(server, Arc::new(Counting::default()));
        let mut silent = TcpStream::connect(addr).await.unwrap();
        assert!(closed_within(&mut silent, Duration::from_secs(2)).await);
        assert_eq!(outcomes.recv().await, Some(false));
    }

    /// A PC that starts pairing and then says nothing more has used one attempt once the
    /// deadline passes.
    #[tokio::test]
    async fn a_pairing_that_runs_out_of_time_is_a_miss() {
        use crate::protocol::{read_message, write_message, Message, PROTOCOL};
        let mut server = Server::bind("127.0.0.1:0".parse().unwrap()).await.unwrap();
        server.deadline = Duration::from_millis(300);
        let addr = server.local_addr();
        let gate = Arc::new(Counting::default());
        let mut outcomes = serve(server, gate.clone());
        let mut s = BufReader::new(TcpStream::connect(addr).await.unwrap());
        write_message(&mut s, &Message::Hello { name: "pc".into(), id: "x".into(), version: PROTOCOL }).await.unwrap();
        read_message(&mut s).await.unwrap();
        write_message(&mut s, &Message::PairStart).await.unwrap();
        assert!(matches!(read_message(&mut s).await.unwrap(), Some(Message::PairKey { .. })));
        assert_eq!(outcomes.recv().await, Some(false));
        assert_eq!(gate.misses.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn one_address_gets_only_its_share_of_handshakes() {
        let server = Server::bind("127.0.0.1:0".parse().unwrap()).await.unwrap();
        let addr = server.local_addr();
        let _outcomes = serve(server, Arc::new(Counting::default()));
        let mut held = Vec::new();
        for _ in 0..MAX_HANDSHAKES_PER_ADDRESS {
            held.push(TcpStream::connect(addr).await.unwrap());
        }
        let mut one_more = TcpStream::connect(addr).await.unwrap();
        assert!(closed_within(&mut one_more, Duration::from_secs(2)).await, "turned away at once");
        // The silent ones are still waiting for their hello.
        assert!(!closed_within(&mut held[0], Duration::from_millis(100)).await);
        // One leaves: its place is free again.
        drop(held.pop());
        tokio::time::sleep(Duration::from_millis(100)).await;
        let mut again = TcpStream::connect(addr).await.unwrap();
        assert!(!closed_within(&mut again, Duration::from_millis(300)).await);
    }

    #[tokio::test]
    async fn the_total_is_capped_too() {
        let mut server = Server::bind("127.0.0.1:0".parse().unwrap()).await.unwrap();
        server.max_total = 2;
        let addr = server.local_addr();
        let _outcomes = serve(server, Arc::new(Counting::default()));
        let _held = [TcpStream::connect(addr).await.unwrap(), TcpStream::connect(addr).await.unwrap()];
        let mut one_more = TcpStream::connect(addr).await.unwrap();
        assert!(closed_within(&mut one_more, Duration::from_secs(2)).await);
    }

    /// Pairing exchanges that run side by side each count once.
    #[tokio::test]
    async fn exchanges_at_the_same_time_each_cost_one_attempt() {
        let server = Server::bind("127.0.0.1:0".parse().unwrap()).await.unwrap();
        let addr = server.local_addr();
        let gate = Arc::new(Counting::default());
        let mut outcomes = serve(server, gate.clone());
        let addrs = [addr];
        let tries = (0..MAX_HANDSHAKES_PER_ADDRESS).map(|_| client::connect(&addrs, "pc", "other", None, Some("000000")));
        let results = futures::future::join_all(tries).await;
        assert!(results.iter().all(|r| matches!(r, Err(LinkError::PairRejected))));
        for _ in 0..MAX_HANDSHAKES_PER_ADDRESS {
            assert_eq!(outcomes.recv().await, Some(false));
        }
        assert_eq!(gate.misses.load(Ordering::SeqCst), MAX_HANDSHAKES_PER_ADDRESS);
    }
}
