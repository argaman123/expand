use async_trait::async_trait;
use sidescreen_core::config::Peer;
use sidescreen_core::link::{
    client, client_handshake, server::Server, server_handshake, Connection, LinkError, TrustPrompt,
    CONNECT_TIMEOUT,
};
use sidescreen_core::protocol::Message;
use std::time::Duration;

struct Answer(bool);

#[async_trait]
impl TrustPrompt for Answer {
    async fn ask(&self, _name: &str, _id: &str) -> bool {
        self.0
    }
}

fn peer(token: &str) -> Peer {
    Peer { name: "pc".into(), id: "pcid".into(), host: "h".into(), token: token.into(), sunshine_paired: false, addresses: Vec::new() }
}

#[tokio::test]
async fn first_connect_pairs_when_prompt_accepts() {
    let (a, b) = tokio::io::duplex(4096);
    let server = tokio::spawn(async move { server_handshake(b, None, &Answer(true)).await });
    let (_, client_peer) = client_handshake(a, "pc", "pcid", None).await.unwrap();
    let (_, server_peer) = server.await.unwrap().unwrap();
    let (c, s) = (client_peer.unwrap(), server_peer.unwrap());
    assert_eq!(c.token.len(), 64);
    assert_eq!(c.token, s.token);
    assert_eq!((s.name.as_str(), s.id.as_str()), ("pc", "pcid"));
}

#[tokio::test]
async fn prompt_rejects_gives_pair_rejected() {
    let (a, b) = tokio::io::duplex(4096);
    let server = tokio::spawn(async move { server_handshake(b, None, &Answer(false)).await.map(|_| ()) });
    let client = client_handshake(a, "pc", "pcid", None).await.map(|_| ());
    assert!(matches!(client, Err(LinkError::PairRejected)));
    assert!(matches!(server.await.unwrap(), Err(LinkError::PairRejected)));
}

#[tokio::test]
async fn known_peer_authenticates_and_exchanges_start_ready() {
    let token = "a".repeat(64);
    let (a, b) = tokio::io::duplex(4096);
    let known = peer(&token);
    let server = tokio::spawn(async move {
        let (stream, new_peer) = server_handshake(b, Some(&known), &Answer(false)).await.unwrap();
        assert!(new_peer.is_none());
        let mut conn = Connection::new(stream);
        assert_eq!(conn.recv().await.unwrap(), Message::Start { width: 2560, height: 1440, fps: 60 });
        conn.send(Message::Ready).await.unwrap();
        conn
    });
    let (stream, new_peer) = client_handshake(a, "pc", "pcid", Some(&peer(&token))).await.unwrap();
    assert!(new_peer.is_none());
    let mut conn = Connection::new(stream);
    conn.send(Message::Start { width: 2560, height: 1440, fps: 60 }).await.unwrap();
    assert_eq!(conn.recv().await.unwrap(), Message::Ready);
    drop(server.await.unwrap());
}

#[tokio::test]
async fn wrong_token_gives_auth_failed() {
    let (a, b) = tokio::io::duplex(4096);
    let known = peer(&"a".repeat(64));
    let server = tokio::spawn(async move { server_handshake(b, Some(&known), &Answer(true)).await.map(|_| ()) });
    let client = client_handshake(a, "pc", "pcid", Some(&peer(&"b".repeat(64)))).await.map(|_| ());
    assert!(matches!(client, Err(LinkError::AuthFailed)));
    assert!(matches!(server.await.unwrap(), Err(LinkError::AuthFailed)));
}

#[tokio::test]
async fn unknown_id_with_a_token_gives_auth_failed() {
    let (a, b) = tokio::io::duplex(4096);
    let server = tokio::spawn(async move { server_handshake(b, None, &Answer(true)).await.map(|_| ()) });
    let client = client_handshake(a, "pc", "pcid", Some(&peer(&"b".repeat(64)))).await.map(|_| ());
    assert!(matches!(client, Err(LinkError::AuthFailed)));
    assert!(matches!(server.await.unwrap(), Err(LinkError::AuthFailed)));
}

type Half = tokio::io::BufReader<tokio::io::DuplexStream>;

async fn authenticated_pair() -> (Half, Half) {
    let token = "a".repeat(64);
    let (a, b) = tokio::io::duplex(4096);
    let known = peer(&token);
    let server = tokio::spawn(async move { server_handshake(b, Some(&known), &Answer(false)).await.unwrap().0 });
    let (client, _) = client_handshake(a, "pc", "pcid", Some(&peer(&token))).await.unwrap();
    (client, server.await.unwrap())
}

#[tokio::test(start_paused = true)]
async fn silent_peer_times_out_after_10s() {
    let (silent_client, server_stream) = authenticated_pair().await;
    let mut server = Connection::new(server_stream);
    let started = tokio::time::Instant::now();
    let result = server.recv().await;
    let waited = started.elapsed();
    assert!(matches!(result, Err(LinkError::Closed)));
    assert!(waited >= Duration::from_secs(9) && waited <= Duration::from_secs(11), "waited {waited:?}");
    drop(silent_client);
}

#[tokio::test(start_paused = true)]
async fn heartbeats_keep_an_idle_link_alive() {
    let (client_stream, server_stream) = authenticated_pair().await;
    let mut client = Connection::new(client_stream);
    let mut server = Connection::new(server_stream);
    let server_task = tokio::spawn(async move { server.recv().await });
    tokio::time::sleep(Duration::from_secs(30)).await;
    client.send(Message::Stop).await.unwrap();
    assert_eq!(server_task.await.unwrap().unwrap(), Message::Stop);
}

#[tokio::test]
async fn peer_closing_gives_closed() {
    let (client_stream, server_stream) = authenticated_pair().await;
    let mut server = Connection::new(server_stream);
    drop(client_stream);
    assert!(matches!(server.recv().await, Err(LinkError::Closed)));
}

#[tokio::test]
async fn nothing_listening_gives_unreachable_within_5s() {
    // A port that was just free on this machine: refused quickly.
    let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
    let started = std::time::Instant::now();
    let r = client::connect(&[free], "pc", "pcid", None).await.map(|_| ());
    assert!(matches!(r, Err(LinkError::Unreachable)));
    assert!(started.elapsed() <= CONNECT_TIMEOUT + Duration::from_secs(1));

    // An address that never answers: must give up at the timeout.
    let started = std::time::Instant::now();
    let r = client::connect(&["10.255.255.1:47800".parse().unwrap()], "pc", "pcid", None).await.map(|_| ());
    assert!(matches!(r, Err(LinkError::Unreachable)));
    assert!(started.elapsed() <= CONNECT_TIMEOUT + Duration::from_secs(1));
}

#[tokio::test]
async fn pairs_and_talks_over_real_tcp() {
    let server = Server::bind("127.0.0.1:0".parse().unwrap()).await.unwrap();
    let addr = server.local_addr();
    let server_task = tokio::spawn(async move {
        let (mut conn, new_peer) = server.accept(|| None, &Answer(true)).await.unwrap();
        assert_eq!(new_peer.unwrap().host, "127.0.0.1");
        conn.send(Message::Ready).await.unwrap();
        conn.recv().await
    });
    let (mut conn, new_peer, _) = client::connect(&[addr], "pc", "pcid", None).await.unwrap();
    assert_eq!(new_peer.unwrap().host, "127.0.0.1");
    assert_eq!(conn.recv().await.unwrap(), Message::Ready);
    conn.send(Message::Stop).await.unwrap();
    assert_eq!(server_task.await.unwrap().unwrap(), Message::Stop);
}

// Review finding: Forget must apply to the very next connection, even though the server was
// already waiting for one when the peer was forgotten.
#[tokio::test]
async fn a_peer_forgotten_while_the_server_waits_is_not_trusted() {
    let token = "a".repeat(64);
    let trusted = std::sync::Arc::new(std::sync::Mutex::new(Some(peer(&token))));
    let server = Server::bind("127.0.0.1:0".parse().unwrap()).await.unwrap();
    let addr = server.local_addr();
    let for_server = trusted.clone();
    let server_task = tokio::spawn(async move {
        server.accept(|| for_server.lock().unwrap().clone(), &Answer(false)).await.map(|_| ())
    });
    tokio::time::sleep(Duration::from_millis(100)).await; // the server is now parked in accept
    *trusted.lock().unwrap() = None;

    let client = client::connect(&[addr], "pc", "pcid", Some(&peer(&token))).await.map(|_| ());
    assert!(matches!(client, Err(LinkError::AuthFailed)));
    assert!(matches!(server_task.await.unwrap(), Err(LinkError::AuthFailed)));
}

// Review finding: if the PC has gone by the time the person clicks Allow, nothing was paired.
#[tokio::test]
async fn pairing_is_not_recorded_when_the_pc_left_before_the_answer() {
    struct SlowYes(tokio::sync::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>);
    #[async_trait]
    impl TrustPrompt for SlowYes {
        async fn ask(&self, _name: &str, _id: &str) -> bool {
            let rx = self.0.lock().await.take().unwrap();
            let _ = rx.await;
            true
        }
    }
    let (release, gate) = tokio::sync::oneshot::channel();
    let (mut a, b) = tokio::io::duplex(4096);
    let server = tokio::spawn(async move {
        server_handshake(b, None, &SlowYes(tokio::sync::Mutex::new(Some(gate)))).await.map(|(_, p)| p)
    });
    // A raw client: say hello, ask to pair, then vanish while the dialog is still up.
    use sidescreen_core::protocol::write_message;
    write_message(&mut a, &Message::Hello { name: "pc".into(), id: "pcid".into() }).await.unwrap();
    write_message(&mut a, &Message::PairRequest { name: "pc".into(), id: "pcid".into(), token: "a".repeat(64) })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    drop(a);
    release.send(()).unwrap();
    assert!(server.await.unwrap().is_err());
}

// 2026-11-18: the PC's saved token no longer matched what the Mac trusted, and the only way out
// was Change → re-enter the address → Connect. A Mac that no longer trusts the PC now gets a fresh
// pairing request (the person at the Mac still has to allow it).
#[tokio::test]
async fn a_peer_the_mac_no_longer_trusts_pairs_again() {
    let server = Server::bind("127.0.0.1:0".parse().unwrap()).await.unwrap();
    let addr = server.local_addr();
    let server_task = tokio::spawn(async move {
        let mac_trusts = || Some(peer(&"a".repeat(64)));
        assert!(matches!(server.accept(mac_trusts, &Answer(true)).await.map(|_| ()), Err(LinkError::AuthFailed)));
        let (_conn, new_peer) = server.accept(mac_trusts, &Answer(true)).await.unwrap();
        new_peer.unwrap()
    });
    let (_conn, new_peer, _) = client::connect_or_pair(&[addr], "pc", "pcid", Some(&peer(&"b".repeat(64)))).await.unwrap();
    let (pc_side, mac_side) = (new_peer.unwrap(), server_task.await.unwrap());
    assert_eq!(pc_side.token, mac_side.token);
    assert_ne!(pc_side.token, "b".repeat(64));
}

#[tokio::test]
async fn a_refused_new_pairing_is_reported_as_refused() {
    let server = Server::bind("127.0.0.1:0".parse().unwrap()).await.unwrap();
    let addr = server.local_addr();
    tokio::spawn(async move {
        let mac_trusts = || Some(peer(&"a".repeat(64)));
        let _ = server.accept(mac_trusts, &Answer(false)).await;
        let _ = server.accept(mac_trusts, &Answer(false)).await;
    });
    let r = client::connect_or_pair(&[addr], "pc", "pcid", Some(&peer(&"b".repeat(64)))).await.map(|_| ());
    assert!(matches!(r, Err(LinkError::PairRejected)));
}

// 2026-11-19: the Mac can be reachable at several addresses (home network, Tailscale, a cable);
// the PC tries them all and takes whichever answers.
#[tokio::test]
async fn the_first_address_that_answers_is_used() {
    let live = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let live_addr = live.local_addr().unwrap();
    let dead = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap(); // freed: refuses
    let unroutable: std::net::SocketAddr = "10.255.255.1:47800".parse().unwrap(); // never answers
    let started = std::time::Instant::now();
    let (_, chosen) = client::first_reachable(&[unroutable, dead, live_addr], CONNECT_TIMEOUT).await.unwrap();
    assert_eq!(chosen, live_addr);
    assert!(started.elapsed() < Duration::from_secs(2), "did not wait for the slow ones: {:?}", started.elapsed());
}

#[tokio::test]
async fn no_address_answering_gives_up_at_the_timeout() {
    let dead = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
    let unroutable: std::net::SocketAddr = "10.255.255.1:47800".parse().unwrap();
    let started = std::time::Instant::now();
    assert!(client::first_reachable(&[dead, unroutable], Duration::from_millis(800)).await.is_none());
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(client::first_reachable(&[], Duration::from_millis(100)).await.is_none());
}
