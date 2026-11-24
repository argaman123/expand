use sidescreen_core::config::Peer;
use sidescreen_core::link::{
    client, client_handshake, server::Server, server_handshake, Connection, LinkError, PairGate,
    CONNECT_TIMEOUT,
};
use sidescreen_core::protocol::{read_message, write_message, Message, PROTOCOL};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};

/// A Mac's pairing mode with a fixed code, which notes how each exchange ended.
struct Code {
    code: &'static str,
    /// Whether pairing mode is open when a PC starts, and still open when it ends.
    open_at_start: bool,
    open_at_end: bool,
    /// `proved` of every exchange concluded, in order.
    ended: Mutex<Vec<bool>>,
}

impl Code {
    fn new(code: &'static str) -> Code {
        Code { code, open_at_start: true, open_at_end: true, ended: Mutex::new(Vec::new()) }
    }

    fn ended(&self) -> Vec<bool> {
        self.ended.lock().unwrap().clone()
    }
}

impl PairGate for Code {
    fn code(&self) -> Option<String> {
        self.open_at_start.then(|| self.code.to_string())
    }

    fn conclude(&self, code: &str, proved: bool) -> bool {
        self.ended.lock().unwrap().push(proved);
        proved && code == self.code && self.open_at_end
    }
}

/// A pairing mode that admits anyone, proved or not, as a broken gate could.
struct SaysYes;

impl PairGate for SaysYes {
    fn code(&self) -> Option<String> {
        Some("123456".to_string())
    }

    fn conclude(&self, _code: &str, _proved: bool) -> bool {
        true
    }
}

const ME: (&str, &str) = ("MacBook", "macid");

fn peer(token: &str) -> Peer {
    Peer { name: "pc".into(), id: "pcid".into(), host: "h".into(), token: token.into(), sunshine_paired: false, addresses: Vec::new(), placement: Default::default(), stream: Default::default(), allowed: true }
}

type PairOutcome = (Result<Option<Peer>, LinkError>, Result<Option<Peer>, LinkError>);

/// Pairs over `pc` and `mac` (the PC with `code`), returning what each side ended with.
async fn pair_over(pc: DuplexStream, mac: DuplexStream, gate: Arc<Code>, code: &str) -> PairOutcome {
    let server = tokio::spawn(async move { server_handshake(mac, None, &*gate, ME).await.map(|(_, p)| p) });
    let client = client_handshake(pc, "Windows PC", "pcid", None, Some(code)).await.map(|(_, p)| p);
    (client, server.await.unwrap())
}

#[tokio::test]
async fn pairing_with_the_right_code_gives_both_sides_the_same_token() {
    let (a, b) = tokio::io::duplex(64 * 1024);
    let gate = Arc::new(Code::new("123456"));
    let (client, server) = pair_over(a, b, gate.clone(), "123456").await;
    let (c, s) = (client.unwrap().unwrap(), server.unwrap().unwrap());
    assert_eq!(c.token.len(), 64);
    assert_eq!(c.token, s.token);
    assert_eq!((s.name.as_str(), s.id.as_str()), ("Windows PC", "pcid"));
    assert_eq!((c.name.as_str(), c.id.as_str()), ("MacBook", "macid"));
    assert_eq!(gate.ended(), [true]);
}

#[tokio::test]
async fn every_pairing_makes_a_new_token() {
    let mut tokens = Vec::new();
    for _ in 0..2 {
        let (a, b) = tokio::io::duplex(64 * 1024);
        let (client, _) = pair_over(a, b, Arc::new(Code::new("123456")), "123456").await;
        tokens.push(client.unwrap().unwrap().token);
    }
    assert_ne!(tokens[0], tokens[1]);
}

#[tokio::test]
async fn pairing_with_a_wrong_code_is_refused_on_both_sides_and_costs_one_attempt() {
    let (a, b) = tokio::io::duplex(64 * 1024);
    let gate = Arc::new(Code::new("123456"));
    let (client, server) = pair_over(a, b, gate.clone(), "654321").await;
    assert!(matches!(client, Err(LinkError::PairRejected)), "{:?}", client.as_ref().map(|_| "a peer"));
    assert!(matches!(server, Err(LinkError::PairRejected)), "{:?}", server.as_ref().map(|_| "a peer"));
    assert_eq!(gate.ended(), [false]);
}

#[tokio::test]
async fn a_closed_pairing_mode_refuses_without_an_exchange() {
    let (a, b) = tokio::io::duplex(64 * 1024);
    let gate = Arc::new(Code { open_at_start: false, ..Code::new("123456") });
    let (client, server) = pair_over(a, b, gate.clone(), "123456").await;
    assert!(matches!(client, Err(LinkError::PairRejected)), "{:?}", client.as_ref().map(|_| "a peer"));
    assert!(matches!(server, Err(LinkError::PairRejected)), "{:?}", server.as_ref().map(|_| "a peer"));
    assert!(gate.ended().is_empty());
}

/// Pairing mode closed (ran out, was cancelled, another PC paired) while the PC was proving
/// itself: the right code is refused, and the PC is given nothing.
#[tokio::test]
async fn a_right_code_is_refused_when_pairing_mode_closed_meanwhile() {
    let (a, b) = tokio::io::duplex(64 * 1024);
    let gate = Arc::new(Code { open_at_end: false, ..Code::new("123456") });
    let (client, server) = pair_over(a, b, gate.clone(), "123456").await;
    assert!(matches!(client, Err(LinkError::PairRejected)), "{:?}", client.as_ref().map(|_| "a peer"));
    assert!(matches!(server, Err(LinkError::PairRejected)), "{:?}", server.as_ref().map(|_| "a peer"));
    assert_eq!(gate.ended(), [true]);
}

#[tokio::test]
async fn a_gate_that_admits_anyone_still_needs_the_code() {
    let (a, b) = tokio::io::duplex(64 * 1024);
    let server = tokio::spawn(async move { server_handshake(b, None, &SaysYes, ME).await.map(|(_, p)| p) });
    let client = client_handshake(a, "pc", "pcid", None, Some("000000")).await.map(|(_, p)| p);
    assert!(matches!(client, Err(LinkError::PairRejected)), "{:?}", client.as_ref().map(|_| "a peer"));
    assert!(matches!(server.await.unwrap(), Err(LinkError::PairRejected)));
}

/// A PC that starts an exchange and goes away has used one attempt.
#[tokio::test]
async fn a_pc_that_leaves_halfway_has_used_an_attempt() {
    let (a, b) = tokio::io::duplex(64 * 1024);
    let gate = Arc::new(Code::new("123456"));
    let for_server = gate.clone();
    let server = tokio::spawn(async move { server_handshake(b, None, &*for_server, ME).await.map(|_| ()) });
    let mut s = BufReader::new(a);
    write_message(&mut s, &Message::Hello { name: "pc".into(), id: "pcid".into(), version: PROTOCOL }).await.unwrap();
    assert!(matches!(read_message(&mut s).await.unwrap(), Some(Message::Challenge { .. })));
    write_message(&mut s, &Message::PairStart).await.unwrap();
    assert!(matches!(read_message(&mut s).await.unwrap(), Some(Message::PairKey { .. })));
    drop(s);
    assert!(server.await.unwrap().is_err());
    assert_eq!(gate.ended(), [false]);
}

/// The PC's first pairing step, as 1.1 sent it: the token in the clear.
#[tokio::test]
async fn the_old_clear_text_pairing_is_not_accepted() {
    let (a, b) = tokio::io::duplex(64 * 1024);
    let gate = Arc::new(Code::new("123456"));
    let for_server = gate.clone();
    let server = tokio::spawn(async move { server_handshake(b, None, &*for_server, ME).await.map(|(_, p)| p) });
    let mut s = BufReader::new(a);
    s.write_all(b"{\"type\":\"hello\",\"name\":\"pc\",\"id\":\"pcid\"}\n").await.unwrap();
    let mut challenge = String::new();
    s.read_line(&mut challenge).await.unwrap();
    let old = format!("{{\"type\":\"pair_request\",\"name\":\"pc\",\"id\":\"pcid\",\"token\":\"{}\",\"proof\":\"00\"}}\n", "a".repeat(64));
    s.write_all(old.as_bytes()).await.unwrap();
    assert!(server.await.unwrap().is_err());
    assert!(gate.ended().is_empty(), "pairing mode was not even asked");
}

/// A PC older than 3 that starts the new exchange anyway (it cannot, but a PC from the future
/// that says another version may): refused as another version, before any attempt.
#[tokio::test]
async fn a_pc_of_another_version_is_refused_as_such() {
    let (a, b) = tokio::io::duplex(64 * 1024);
    let gate = Arc::new(Code::new("123456"));
    let for_server = gate.clone();
    let server = tokio::spawn(async move { server_handshake(b, None, &*for_server, ME).await.map(|_| ()) });
    let mut s = BufReader::new(a);
    write_message(&mut s, &Message::Hello { name: "pc".into(), id: "pcid".into(), version: PROTOCOL + 1 }).await.unwrap();
    read_message(&mut s).await.unwrap();
    write_message(&mut s, &Message::PairStart).await.unwrap();
    assert!(matches!(read_message(&mut s).await.unwrap(), Some(Message::PairResult { accepted: false, .. })));
    assert!(matches!(server.await.unwrap(), Err(LinkError::Version)));
    assert!(gate.ended().is_empty());
}

/// A Mac older than 3 would take the token in the clear: the PC says it must be updated, and
/// sends nothing after its hello.
#[tokio::test]
async fn an_older_mac_is_not_paired_with() {
    let (a, b) = tokio::io::duplex(64 * 1024);
    let mac = tokio::spawn(async move {
        let mut s = BufReader::new(b);
        let _hello = read_message(&mut s).await;
        s.write_all(b"{\"type\":\"challenge\",\"nonce\":\"00\"}\n").await.unwrap();
        read_message(&mut s).await
    });
    let client = client_handshake(a, "pc", "pcid", None, Some("123456")).await.map(|_| ());
    assert!(matches!(client, Err(LinkError::Version)), "{:?}", client.as_ref().map(|_| "a peer"));
    assert_eq!(LinkError::Version.to_string(), "the other computer runs another version of Sidescreen; update it");
    assert!(matches!(mac.await.unwrap(), Ok(None)), "the PC said more");
}

/// Something on the network that says "paired" without having the code.
#[tokio::test]
async fn a_mac_that_cannot_prove_the_code_is_not_paired_with() {
    use spake2::{Ed25519Group, Identity, Password, Spake2};
    let (a, b) = tokio::io::duplex(64 * 1024);
    tokio::spawn(async move {
        let mut s = BufReader::new(b);
        let _hello = read_message(&mut s).await;
        write_message(&mut s, &Message::Challenge { nonce: "00".into(), version: PROTOCOL }).await.unwrap();
        let _start = read_message(&mut s).await;
        let (_, pake) = Spake2::<Ed25519Group>::start_b(
            &Password::new(b"000000"),
            &Identity::new(b"sidescreen-pc:pcid"),
            &Identity::new(b"sidescreen-mac:macid"),
        );
        let key = Message::PairKey { id: "macid".into(), name: "MacBook".into(), pake: hex::encode(pake) };
        write_message(&mut s, &key).await.unwrap();
        let _confirm = read_message(&mut s).await;
        let answer = Message::PairResult { accepted: true, confirm: "00".repeat(32), token: "00".repeat(80) };
        write_message(&mut s, &answer).await.unwrap();
    });
    let client = client_handshake(a, "Windows PC", "pcid", None, Some("123456")).await.map(|_| ());
    assert!(matches!(client, Err(LinkError::PairRejected)), "{:?}", client.as_ref().map(|_| "a peer"));
}

#[tokio::test]
async fn a_pair_result_without_a_key_exchange_is_a_rejection() {
    let (a, b) = tokio::io::duplex(64 * 1024);
    tokio::spawn(async move {
        let mut s = BufReader::new(b);
        let _hello = read_message(&mut s).await;
        write_message(&mut s, &Message::Challenge { nonce: "00".into(), version: PROTOCOL }).await.unwrap();
        let _start = read_message(&mut s).await;
        let answer = Message::PairResult { accepted: true, confirm: String::new(), token: "a".repeat(64) };
        write_message(&mut s, &answer).await.unwrap();
    });
    let client = client_handshake(a, "Windows PC", "pcid", None, Some("123456")).await.map(|_| ());
    assert!(matches!(client, Err(LinkError::PairRejected)), "{:?}", client.as_ref().map(|_| "a peer"));
}

// ---- a man in the middle ----

type Wire = Arc<Mutex<Vec<u8>>>;

/// A relay between a PC (the first stream) and a Mac (the second) that records every byte
/// either sends and passes each message through `tamper` on its way.
fn middle(tamper: impl Fn(&mut Message) + Send + Sync + 'static) -> (DuplexStream, DuplexStream, Wire) {
    let (pc, pc_far) = tokio::io::duplex(64 * 1024);
    let (mac_far, mac) = tokio::io::duplex(64 * 1024);
    let tamper = Arc::new(tamper);
    let wire: Wire = Arc::default();
    let (pc_r, pc_w) = tokio::io::split(pc_far);
    let (mac_r, mac_w) = tokio::io::split(mac_far);
    tokio::spawn(pump(pc_r, mac_w, tamper.clone(), wire.clone()));
    tokio::spawn(pump(mac_r, pc_w, tamper, wire.clone()));
    (pc, mac, wire)
}

async fn pump<R, W>(r: R, mut w: W, tamper: Arc<impl Fn(&mut Message)>, wire: Wire)
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut r = BufReader::new(r);
    loop {
        let mut line = Vec::new();
        if r.read_until(b'\n', &mut line).await.unwrap_or(0) == 0 {
            break;
        }
        wire.lock().unwrap().extend_from_slice(&line);
        let mut m: Message = serde_json::from_slice(&line).unwrap();
        tamper(&mut m);
        if write_message(&mut w, &m).await.is_err() {
            break;
        }
    }
    let _ = w.shutdown().await;
}

/// Changes the last hex digit of `s`.
fn flip(s: &mut String) {
    let last = s.pop().unwrap();
    s.push(if last == '0' { '1' } else { '0' });
}

#[tokio::test]
async fn a_recording_of_the_pairing_holds_neither_the_token_nor_the_code() {
    let (pc, mac, wire) = middle(|_| {});
    let (client, server) = pair_over(pc, mac, Arc::new(Code::new("907153")), "907153").await;
    let token = client.unwrap().unwrap().token;
    assert_eq!(server.unwrap().unwrap().token, token);
    let wire = wire.lock().unwrap().clone();
    let text = String::from_utf8(wire.clone()).unwrap();
    assert!(text.contains("pair_key") && text.contains("pair_confirm") && text.contains("pair_result"), "{text}");
    assert!(!text.to_lowercase().contains(&token), "the token crossed the wire");
    let raw = hex::decode(&token).unwrap();
    assert!(!wire.windows(raw.len()).any(|w| w == raw.as_slice()), "the token's bytes crossed the wire");
    assert!(!text.contains("\"907153\""), "the code crossed the wire");
}

/// Each change a man in the middle could make, and whether the Mac (which commits first) can
/// still notice it: everything but the Mac's own last message.
#[tokio::test]
async fn a_changed_message_is_refused() {
    type Change = fn(&mut Message);
    let cases: [(&str, Change, bool); 10] = [
        ("hello name", |m| if let Message::Hello { name, .. } = m { name.push('!') }, true),
        ("hello id", |m| if let Message::Hello { id, .. } = m { id.push('!') }, true),
        ("challenge nonce", |m| if let Message::Challenge { nonce, .. } = m { flip(nonce) }, true),
        ("mac's pake", |m| if let Message::PairKey { pake, .. } = m { flip(pake) }, true),
        ("mac's name", |m| if let Message::PairKey { name, .. } = m { name.push('!') }, true),
        ("mac's id", |m| if let Message::PairKey { id, .. } = m { id.push('!') }, true),
        ("pc's pake", |m| if let Message::PairConfirm { pake, .. } = m { flip(pake) }, true),
        ("pc's confirmation", |m| if let Message::PairConfirm { confirm, .. } = m { flip(confirm) }, true),
        ("mac's confirmation", |m| if let Message::PairResult { confirm, .. } = m { flip(confirm) }, false),
        ("sealed token", |m| if let Message::PairResult { token, .. } = m { flip(token) }, false),
    ];
    for (what, change, mac_notices) in cases {
        let (pc, mac, _) = middle(change);
        let gate = Arc::new(Code::new("123456"));
        let (client, server) = pair_over(pc, mac, gate.clone(), "123456").await;
        assert!(matches!(client, Err(LinkError::PairRejected)), "{what}: the PC took {:?}", client.map(|_| "a peer"));
        if mac_notices {
            assert!(server.is_err(), "{what}: the Mac took a peer");
            assert_eq!(gate.ended(), [false], "{what}");
        }
    }
}

#[tokio::test]
async fn without_a_token_or_a_code_the_pc_is_told_it_needs_a_code() {
    let (a, _b) = tokio::io::duplex(4096);
    let client = client_handshake(a, "pc", "pcid", None, None).await.map(|_| ());
    assert!(matches!(client, Err(LinkError::NeedsCode)));
    assert_eq!(LinkError::NeedsCode.to_string(), "This Mac needs its pairing code");
    // A peer with an empty token counts as no token.
    let (a, _b) = tokio::io::duplex(4096);
    assert!(matches!(client_handshake(a, "pc", "pcid", Some(&peer("")), None).await.map(|_| ()), Err(LinkError::NeedsCode)));
}

#[tokio::test]
async fn a_known_pc_with_a_stale_token_gets_auth_failed_and_is_not_re_paired() {
    let mut server = Server::bind("127.0.0.1:0".parse().unwrap()).await.unwrap();
    let addr = server.local_addr();
    let server_task = tokio::spawn(async move {
        let mac_trusts = |_: &str| Some(peer(&"a".repeat(64)));
        server.accept(mac_trusts, Arc::new(Code::new("123456")), ME).await.map(|_| ())
    });
    // Even with the right code at hand, a stale token is only ever AuthFailed.
    let r = client::connect(&[addr], "pc", "pcid", Some(&peer(&"b".repeat(64))), Some("123456")).await.map(|_| ());
    assert!(matches!(r, Err(LinkError::AuthFailed)));
    assert!(matches!(server_task.await.unwrap(), Err(LinkError::AuthFailed)));
}

#[tokio::test]
async fn a_blocked_pc_is_refused_like_an_unknown_one() {
    let token = "a".repeat(64);
    let mut server = Server::bind("127.0.0.1:0".parse().unwrap()).await.unwrap();
    let addr = server.local_addr();
    let blocked = Peer { allowed: false, ..peer(&token) };
    let server_task = tokio::spawn(async move {
        // The lookup is by PC id and withholds a PC that is not allowed.
        let lookup = move |id: &str| (id == blocked.id && blocked.allowed).then(|| blocked.clone());
        server.accept(lookup, Arc::new(Code::new("123456")), ME).await.map(|_| ())
    });
    let r = client::connect(&[addr], "pc", "pcid", Some(&peer(&token)), None).await.map(|_| ());
    assert!(matches!(r, Err(LinkError::AuthFailed)));
    assert!(matches!(server_task.await.unwrap(), Err(LinkError::AuthFailed)));
}

#[tokio::test]
async fn known_peer_authenticates_and_exchanges_start_ready() {
    let token = "a".repeat(64);
    let (a, b) = tokio::io::duplex(4096);
    let known = peer(&token);
    let server = tokio::spawn(async move {
        let (stream, new_peer) = server_handshake(b, Some(&known), &Code::new("0"), ME).await.unwrap();
        assert!(new_peer.is_none());
        let mut conn = Connection::new(stream);
        assert_eq!(conn.recv().await.unwrap(), Message::Start { width: 2560, height: 1440, fps: 60 });
        conn.send(Message::Ready).await.unwrap();
        conn
    });
    let (stream, new_peer) = client_handshake(a, "pc", "pcid", Some(&peer(&token)), None).await.unwrap();
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
    let server = tokio::spawn(async move { server_handshake(b, Some(&known), &Code::new("0"), ME).await.map(|_| ()) });
    let client = client_handshake(a, "pc", "pcid", Some(&peer(&"b".repeat(64))), None).await.map(|_| ());
    assert!(matches!(client, Err(LinkError::AuthFailed)));
    assert!(matches!(server.await.unwrap(), Err(LinkError::AuthFailed)));
}

#[tokio::test]
async fn unknown_id_with_a_token_gives_auth_failed() {
    let (a, b) = tokio::io::duplex(4096);
    let server = tokio::spawn(async move { server_handshake(b, None, &Code::new("0"), ME).await.map(|_| ()) });
    let client = client_handshake(a, "pc", "pcid", Some(&peer(&"b".repeat(64))), None).await.map(|_| ());
    assert!(matches!(client, Err(LinkError::AuthFailed)));
    assert!(matches!(server.await.unwrap(), Err(LinkError::AuthFailed)));
}

type Half = tokio::io::BufReader<tokio::io::DuplexStream>;

async fn authenticated_pair() -> (Half, Half) {
    let token = "a".repeat(64);
    let (a, b) = tokio::io::duplex(4096);
    let known = peer(&token);
    let server = tokio::spawn(async move { server_handshake(b, Some(&known), &Code::new("0"), ME).await.unwrap().0 });
    let (client, _) = client_handshake(a, "pc", "pcid", Some(&peer(&token)), None).await.unwrap();
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
    let r = client::connect(&[free], "pc", "pcid", None, None).await.map(|_| ());
    assert!(matches!(r, Err(LinkError::Unreachable)));
    assert!(started.elapsed() <= CONNECT_TIMEOUT + Duration::from_secs(1));

    // An address that never answers: must give up at the timeout.
    let started = std::time::Instant::now();
    let r = client::connect(&["10.255.255.1:47800".parse().unwrap()], "pc", "pcid", None, None).await.map(|_| ());
    assert!(matches!(r, Err(LinkError::Unreachable)));
    assert!(started.elapsed() <= CONNECT_TIMEOUT + Duration::from_secs(1));
}

#[tokio::test]
async fn pairs_and_talks_over_real_tcp() {
    let mut server = Server::bind("127.0.0.1:0".parse().unwrap()).await.unwrap();
    let addr = server.local_addr();
    let server_task = tokio::spawn(async move {
        let (mut conn, new_peer, pc_id) = server.accept(|_: &str| None, Arc::new(Code::new("123456")), ME).await.unwrap();
        assert_eq!(new_peer.unwrap().host, "127.0.0.1");
        assert_eq!(pc_id, "pcid");
        conn.send(Message::Ready).await.unwrap();
        conn.recv().await
    });
    let (mut conn, new_peer, _) = client::connect(&[addr], "pc", "pcid", None, Some("123456")).await.unwrap();
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
    let mut server = Server::bind("127.0.0.1:0".parse().unwrap()).await.unwrap();
    let addr = server.local_addr();
    let for_server = trusted.clone();
    let server_task = tokio::spawn(async move {
        server.accept(move |_: &str| for_server.lock().unwrap().clone(), Arc::new(Code::new("0")), ME).await.map(|_| ())
    });
    tokio::time::sleep(Duration::from_millis(100)).await; // the server is now parked in accept
    *trusted.lock().unwrap() = None;

    let client = client::connect(&[addr], "pc", "pcid", Some(&peer(&token)), None).await.map(|_| ());
    assert!(matches!(client, Err(LinkError::AuthFailed)));
    assert!(matches!(server_task.await.unwrap(), Err(LinkError::AuthFailed)));
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
