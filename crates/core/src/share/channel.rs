//! The connection mouse and keyboard travel over: TCP, authenticated with the pairing token and
//! encrypted, so keystrokes never cross the network in the clear.
//!
//! Handshake: the PC sends `{"id", "nonce", "version"}` as a line, the Mac answers `{"nonce", "id", "name",
//! "version"}`, or `{"refused": ...}` when it turns the PC away. Each side derives one key per direction from the
//! token and both nonces (HKDF-SHA256), then the PC proves it has the token with a first encrypted frame,
//! and the Mac answers with its own proof, or (version 2) an encrypted refusal when another PC took
//! it meanwhile.
//! Frames: 4-byte big-endian length, then ChaCha20-Poly1305 ciphertext of a JSON message; the
//! nonce is a per-direction counter, so a frame cannot be replayed or reordered.

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::io;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

/// The Mac listens for mouse and keyboard here (one above the control link's port).
pub const INPUT_PORT: u16 = crate::link::CONTROL_PORT + 1;

/// Largest frame accepted: input messages are tiny; a copied image can be a few megabytes.
const MAX_FRAME: usize = 48 * 1024 * 1024;
const PROOF: &[u8] = b"expand input";

/// The handshake's version, Expand's protocol version ([`crate::protocol::PROTOCOL`]).
/// 2: both hellos say their version, and the Mac claims its slot only after the PC's proof
/// (answering with a refusal there when it lost a race). 3: nothing changed here, but pairing
/// did, so a PC and a Mac that cannot pair do not share either. A side whose hello has no
/// version is version 1. Versions that differ are refused, never retried.
pub use crate::protocol::PROTOCOL;

fn version_one() -> u32 {
    1
}

pub struct Sender<W> {
    out: W,
    cipher: ChaCha20Poly1305,
    counter: u64,
}

pub struct Receiver<R> {
    inp: R,
    cipher: ChaCha20Poly1305,
    counter: u64,
}

impl<W: AsyncWrite + Unpin> Sender<W> {
    pub async fn send<T: Serialize>(&mut self, message: &T) -> io::Result<()> {
        let plain = serde_json::to_vec(message).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let sealed = self
            .cipher
            .encrypt(&nonce(self.counter), plain.as_ref())
            .map_err(|_| io::Error::other("encryption failed"))?;
        self.counter += 1;
        // One write per frame, so a frame never reaches the other side in pieces by our doing.
        let mut frame = Vec::with_capacity(4 + sealed.len());
        frame.extend_from_slice(&(sealed.len() as u32).to_be_bytes());
        frame.extend_from_slice(&sealed);
        self.out.write_all(&frame).await?;
        self.out.flush().await
    }

    /// Closes the sending side, so the other end sees the connection end now rather than when
    /// it next writes.
    pub async fn close(&mut self) -> io::Result<()> {
        self.out.shutdown().await
    }
}

impl<R: AsyncRead + Unpin> Receiver<R> {
    pub async fn recv<T: DeserializeOwned>(&mut self) -> io::Result<T> {
        let mut len = [0u8; 4];
        self.inp.read_exact(&mut len).await?;
        let len = u32::from_be_bytes(len) as usize;
        if len > MAX_FRAME {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "frame too large"));
        }
        let mut sealed = vec![0u8; len];
        self.inp.read_exact(&mut sealed).await?;
        let plain = self
            .cipher
            .decrypt(&nonce(self.counter), sealed.as_ref())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "frame failed authentication"))?;
        self.counter += 1;
        serde_json::from_slice(&plain).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
}

impl<R: AsyncRead + Unpin + Send + 'static> Receiver<R> {
    /// Reads every frame in a task of its own and hands them over in order. `recv` on the
    /// queue can be raced in `select!` safely; reading frames directly cannot (a frame read
    /// half-way would be lost). The last item is the error that ended the connection.
    pub fn into_queue<T: DeserializeOwned + Send + 'static>(mut self) -> tokio::sync::mpsc::Receiver<io::Result<T>> {
        let (tx, rx) = tokio::sync::mpsc::channel(256);
        tokio::spawn(async move {
            loop {
                let item = self.recv::<T>().await;
                let failed = item.is_err();
                if tx.send(item).await.is_err() || failed {
                    break;
                }
            }
        });
        rx
    }
}

pub type Halves<S> = (Receiver<BufReader<tokio::io::ReadHalf<S>>>, Sender<tokio::io::WriteHalf<S>>);

/// Why a Mac turned a PC away at the door.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "refused", rename_all = "snake_case")]
pub enum Refusal {
    /// Another PC is connected; `by` is its name.
    Busy { by: String },
    /// The Mac's user blocked this PC.
    Blocked,
    /// The Mac does not know this PC.
    Unknown,
    /// The two computers run versions of Expand that cannot work together.
    Version,
}

/// The Mac's decision about a PC that says who it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admit {
    Accept { token: String },
    Refuse(Refusal),
}

/// Who a Mac says it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MacIdentity {
    pub id: String,
    pub name: String,
}

#[derive(Debug)]
pub enum ShareHandshakeError {
    Refused(Refusal),
    /// The PC proved itself and the Mac then closed the connection (or answered with another
    /// key) instead of proving itself: it does not hold this PC's token any more.
    Unproven,
    Io(io::Error),
}

impl From<io::Error> for ShareHandshakeError {
    fn from(e: io::Error) -> Self {
        ShareHandshakeError::Io(e)
    }
}

impl std::fmt::Display for ShareHandshakeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShareHandshakeError::Refused(Refusal::Busy { by }) => write!(f, "the Mac is in use by {by}"),
            ShareHandshakeError::Refused(Refusal::Blocked) => write!(f, "the Mac has blocked this PC"),
            ShareHandshakeError::Refused(Refusal::Unknown) => write!(f, "the Mac does not know this PC"),
            ShareHandshakeError::Refused(Refusal::Version) => write!(f, "the other computer runs another version of Expand"),
            ShareHandshakeError::Unproven => write!(f, "the Mac did not accept this PC's pairing"),
            ShareHandshakeError::Io(e) => write!(f, "{e}"),
        }
    }
}

/// PC side of the handshake.
pub async fn pc_handshake<S: AsyncRead + AsyncWrite>(
    stream: S,
    my_id: &str,
    token: &str,
) -> Result<(Halves<S>, MacIdentity), ShareHandshakeError> {
    let (read, mut write) = tokio::io::split(stream);
    let mut read = BufReader::new(read);
    let pc_nonce: [u8; 16] = rand::random();
    write_line(&mut write, &PcHello { id: my_id.to_string(), nonce: hex::encode(pc_nonce), version: PROTOCOL }).await?;
    let answer = match read_line::<_, Reply>(&mut read).await? {
        Reply::Refused(why) => return Err(ShareHandshakeError::Refused(why)),
        Reply::Hello(hello) => hello,
    };
    // An older Mac says no version and would never replay input the way this PC sends it.
    if answer.version != PROTOCOL {
        return Err(ShareHandshakeError::Refused(Refusal::Version));
    }
    let mac_nonce = hex::decode(&answer.nonce).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let (to_mac, to_pc) = derive(token, &pc_nonce, &mac_nonce);
    let mut sender = Sender { out: write, cipher: to_mac, counter: 0 };
    sender.send(&String::from_utf8_lossy(PROOF)).await?;
    // The Mac proves it holds the token too, before anything is captured and sent its way. A
    // Mac with another token cannot read the PC's proof and closes (however the OS reports
    // that), or answers with a frame this PC cannot read: either way it is unproven.
    let mut receiver = Receiver { inp: read, cipher: to_pc, counter: 0 };
    match receiver.recv::<MacAnswer>().await {
        Ok(MacAnswer::Proof(proof)) if proof.as_bytes() == PROOF => {}
        // Proved, but another PC took the Mac first.
        Ok(MacAnswer::Refused(why)) => return Err(ShareHandshakeError::Refused(why)),
        _ => return Err(ShareHandshakeError::Unproven),
    }
    Ok(((receiver, sender), MacIdentity { id: answer.id, name: answer.name }))
}

/// Mac side of the handshake. `decide` takes the PC's id and says whether to let it in (with its
/// token) or why not. Returns the connection and the PC's id.
pub async fn mac_handshake<S: AsyncRead + AsyncWrite>(
    stream: S,
    me: &MacIdentity,
    decide: impl Fn(&str) -> Admit,
) -> io::Result<(Halves<S>, String)> {
    mac_handshake_claiming(stream, me, decide, |_| Ok(())).await
}

/// [`mac_handshake`], with `claim` asked once the PC has proved it holds the token: an `Err`
/// is sent to the PC as the reason it is turned away after all (another PC got in first).
pub async fn mac_handshake_claiming<S: AsyncRead + AsyncWrite>(
    stream: S,
    me: &MacIdentity,
    decide: impl Fn(&str) -> Admit,
    claim: impl FnOnce(&str) -> Result<(), Refusal>,
) -> io::Result<(Halves<S>, String)> {
    let (read, mut write) = tokio::io::split(stream);
    let mut read = BufReader::new(read);
    let hello: PcHello = read_line(&mut read).await?;
    if hello.version != PROTOCOL {
        write_line(&mut write, &Refusal::Version).await?;
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "the PC runs another version"));
    }
    let token = match decide(&hello.id) {
        Admit::Accept { token } => token,
        Admit::Refuse(why) => {
            write_line(&mut write, &why).await?;
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "the PC was refused"));
        }
    };
    let pc_nonce = hex::decode(&hello.nonce).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let mac_nonce: [u8; 16] = rand::random();
    write_line(&mut write, &MacHello { nonce: hex::encode(mac_nonce), id: me.id.clone(), name: me.name.clone(), version: PROTOCOL })
        .await?;
    let (to_mac, to_pc) = derive(&token, &pc_nonce, &mac_nonce);
    let mut receiver = Receiver { inp: read, cipher: to_mac, counter: 0 };
    let proof: String = receiver.recv().await?;
    if proof.as_bytes() != PROOF {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "wrong proof"));
    }
    let mut sender = Sender { out: write, cipher: to_pc, counter: 0 };
    if let Err(why) = claim(&hello.id) {
        sender.send(&why).await?;
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "the PC was refused after its proof"));
    }
    sender.send(&String::from_utf8_lossy(PROOF)).await?;
    Ok(((receiver, sender), hello.id))
}

fn nonce(counter: u64) -> Nonce {
    let mut n = [0u8; 12];
    n[..8].copy_from_slice(&counter.to_le_bytes());
    *Nonce::from_slice(&n)
}

/// One key for each direction, from the shared token and both sides' fresh nonces.
fn derive(token: &str, pc_nonce: &[u8], mac_nonce: &[u8]) -> (ChaCha20Poly1305, ChaCha20Poly1305) {
    let salt = [pc_nonce, mac_nonce].concat();
    let hk = hkdf::Hkdf::<sha2::Sha256>::new(Some(&salt), token.as_bytes());
    let (mut to_mac, mut to_pc) = ([0u8; 32], [0u8; 32]);
    hk.expand(b"expand input pc to mac", &mut to_mac).expect("32 bytes is a valid length");
    hk.expand(b"expand input mac to pc", &mut to_pc).expect("32 bytes is a valid length");
    (ChaCha20Poly1305::new(Key::from_slice(&to_mac)), ChaCha20Poly1305::new(Key::from_slice(&to_pc)))
}

async fn write_line<W: AsyncWrite + Unpin, T: Serialize>(out: &mut W, value: &T) -> io::Result<()> {
    let mut line = serde_json::to_vec(value).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    line.push(b'\n');
    out.write_all(&line).await?;
    out.flush().await
}

async fn read_line<R: AsyncBufReadExt + Unpin, T: DeserializeOwned>(inp: &mut R) -> io::Result<T> {
    let mut line = String::new();
    let n = (&mut *inp).take(1024).read_line(&mut line).await?;
    if n == 0 {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "closed during the handshake"));
    }
    serde_json::from_str(line.trim_end()).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[derive(Serialize, Deserialize)]
struct PcHello {
    id: String,
    nonce: String,
    #[serde(default = "version_one")]
    version: u32,
}

#[derive(Serialize, Deserialize)]
struct MacHello {
    nonce: String,
    /// Missing from a Mac app older than the redesign.
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default = "version_one")]
    version: u32,
}

/// The Mac's first encrypted frame: its proof, or why it turns the proved PC away after all.
#[derive(Deserialize)]
#[serde(untagged)]
enum MacAnswer {
    Proof(String),
    Refused(Refusal),
}

/// The Mac's one-line answer: let in, or turned away.
#[derive(Deserialize)]
#[serde(untagged)]
enum Reply {
    Refused(Refusal),
    Hello(MacHello),
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn trusted(id: &str) -> Admit {
        if id == "pc" {
            Admit::Accept { token: TOKEN.to_string() }
        } else {
            Admit::Refuse(Refusal::Unknown)
        }
    }

    fn mac() -> MacIdentity {
        MacIdentity { id: "mac-1".into(), name: "Studio".into() }
    }

    async fn connected() -> (Halves<tokio::io::DuplexStream>, Halves<tokio::io::DuplexStream>) {
        let (a, b) = tokio::io::duplex(64 * 1024);
        let mac = tokio::spawn(async move { mac_handshake(b, &mac(), trusted).await.unwrap() });
        let (pc, _) = pc_handshake(a, "pc", TOKEN).await.unwrap();
        (pc, mac.await.unwrap().0)
    }

    #[tokio::test]
    async fn messages_go_both_ways() {
        let ((mut pc_in, mut pc_out), (mut mac_in, mut mac_out)) = connected().await;
        pc_out.send(&("move", 3, -4)).await.unwrap();
        pc_out.send(&("key", 30)).await.unwrap();
        assert_eq!(mac_in.recv::<(String, i32, i32)>().await.unwrap(), ("move".to_string(), 3, -4));
        assert_eq!(mac_in.recv::<(String, i32)>().await.unwrap(), ("key".to_string(), 30));
        mac_out.send(&"back").await.unwrap();
        assert_eq!(pc_in.recv::<String>().await.unwrap(), "back");
    }

    #[tokio::test]
    async fn a_pc_with_the_wrong_token_is_turned_away() {
        let (a, b) = tokio::io::duplex(64 * 1024);
        let mac = tokio::spawn(async move { mac_handshake(b, &mac(), trusted).await.map(|_| ()) });
        let _ = pc_handshake(a, "pc", &"b".repeat(64)).await;
        assert!(mac.await.unwrap().is_err());
    }

    #[tokio::test]
    async fn an_unknown_pc_is_turned_away() {
        let (a, b) = tokio::io::duplex(64 * 1024);
        let mac = tokio::spawn(async move { mac_handshake(b, &mac(), trusted).await.map(|_| ()) });
        let _ = pc_handshake(a, "someone else", TOKEN).await;
        assert!(mac.await.unwrap().is_err());
    }

    #[tokio::test]
    async fn keystrokes_are_not_readable_on_the_wire() {
        let (a, b) = tokio::io::duplex(64 * 1024);
        let (tap_in, mut tap_out) = tokio::io::duplex(64 * 1024);
        // Everything the PC writes is copied to a tap before reaching the Mac.
        let (mut a_read, mut a_write) = tokio::io::split(a);
        let (mut b_read, mut b_write) = tokio::io::split(b);
        let mac = tokio::spawn(async move {
            let (bridge_a, bridge_b) = tokio::io::duplex(64 * 1024);
            let (mut bridge_read, mut bridge_write) = tokio::io::split(bridge_a);
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                loop {
                    let n = b_read.read(&mut buf).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    tap_out.write_all(&buf[..n]).await.unwrap();
                    bridge_write.write_all(&buf[..n]).await.unwrap();
                }
            });
            tokio::spawn(async move { tokio::io::copy(&mut bridge_read, &mut b_write).await });
            mac_handshake(bridge_b, &mac(), trusted).await.unwrap()
        });
        let pc_stream = tokio::io::join(&mut a_read, &mut a_write);
        let ((_, mut pc_out), _) = pc_handshake(pc_stream, "pc", TOKEN).await.unwrap();
        pc_out.send(&"my secret password").await.unwrap();
        let ((mut mac_in, _), _) = mac.await.unwrap();
        assert_eq!(mac_in.recv::<String>().await.unwrap(), "my secret password");
        drop(pc_out);
        let mut seen = Vec::new();
        let mut tap_in = tap_in;
        let _ = tokio::time::timeout(std::time::Duration::from_millis(200), tap_in.read_to_end(&mut seen)).await;
        assert!(!String::from_utf8_lossy(&seen).contains("secret"));
    }

    // Review finding: the PC armed its capture before the Mac had shown it holds the token.
    #[tokio::test]
    async fn a_mac_that_does_not_know_the_token_is_found_out() {
        let (a, b) = tokio::io::duplex(64 * 1024);
        // Something at the Mac's address that answers the handshake without the real token.
        tokio::spawn(async move { mac_handshake(b, &mac(), |_| Admit::Accept { token: "b".repeat(64) }).await.map(|_| ()) });
        assert!(matches!(pc_handshake(a, "pc", TOKEN).await, Err(ShareHandshakeError::Unproven)));
    }

    // Review finding: a frame read half-way and then abandoned in a select! desynced the stream.
    #[tokio::test]
    async fn the_queue_delivers_every_frame_in_order() {
        let ((_, mut pc_out), (mac_in, _)) = connected().await;
        let mut queue = mac_in.into_queue::<u32>();
        for n in 0..200u32 {
            pc_out.send(&n).await.unwrap();
        }
        for n in 0..200u32 {
            // Racing the queue against a timer must never lose a frame.
            let got = loop {
                tokio::select! {
                    m = queue.recv() => break m.unwrap().unwrap(),
                    _ = tokio::task::yield_now() => {}
                }
            };
            assert_eq!(got, n);
        }
        drop(pc_out);
        assert!(queue.recv().await.unwrap().is_err(), "the end of the connection is reported");
    }

    #[tokio::test]
    async fn a_tampered_frame_is_rejected() {
        let (a, b) = tokio::io::duplex(64 * 1024);
        let mac = tokio::spawn(async move { mac_handshake(b, &mac(), trusted).await.unwrap() });
        let ((_, mut pc_out), _) = pc_handshake(a, "pc", TOKEN).await.unwrap();
        let ((mut mac_in, _), _) = mac.await.unwrap();
        // Write a frame of the right length but garbage content straight onto the wire.
        let garbage = vec![0x5a; 40];
        pc_out.out.write_all(&(garbage.len() as u32).to_be_bytes()).await.unwrap();
        pc_out.out.write_all(&garbage).await.unwrap();
        assert!(mac_in.recv::<String>().await.is_err());
    }

    async fn refused_with(decision: Admit) -> (Result<(), ShareHandshakeError>, io::Result<()>) {
        let (a, b) = tokio::io::duplex(64 * 1024);
        let mac = tokio::spawn(async move { mac_handshake(b, &mac(), move |_| decision.clone()).await.map(|_| ()) });
        let pc = pc_handshake(a, "pc", TOKEN).await.map(|_| ());
        (pc, mac.await.unwrap())
    }

    #[tokio::test]
    async fn the_pc_learns_the_macs_identity() {
        let (a, b) = tokio::io::duplex(64 * 1024);
        let mac_task = tokio::spawn(async move { mac_handshake(b, &mac(), trusted).await.unwrap().1 });
        let (_, who) = pc_handshake(a, "pc", TOKEN).await.unwrap();
        assert_eq!(who, MacIdentity { id: "mac-1".into(), name: "Studio".into() });
        assert_eq!(mac_task.await.unwrap(), "pc");
    }

    // Ledger T6: the Mac claims its slot only after the PC's proof, and a PC that lost the race
    // there is still told who has the Mac.
    #[tokio::test]
    async fn a_pc_that_cannot_prove_itself_is_never_claimed_for() {
        let (a, b) = tokio::io::duplex(64 * 1024);
        let claimed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen = claimed.clone();
        let mac = tokio::spawn(async move {
            mac_handshake_claiming(b, &mac(), trusted, move |_| {
                seen.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            })
            .await
            .map(|_| ())
        });
        let _ = pc_handshake(a, "pc", &"b".repeat(64)).await;
        assert!(mac.await.unwrap().is_err());
        assert!(!claimed.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn a_pc_that_loses_the_race_after_its_proof_is_told_busy() {
        let (a, b) = tokio::io::duplex(64 * 1024);
        let mac = tokio::spawn(async move {
            mac_handshake_claiming(b, &mac(), trusted, |_| Err(Refusal::Busy { by: "Desk".into() })).await.map(|_| ())
        });
        let pc = pc_handshake(a, "pc", TOKEN).await.map(|_| ());
        assert!(matches!(pc, Err(ShareHandshakeError::Refused(Refusal::Busy { by })) if by == "Desk"));
        assert!(mac.await.unwrap().is_err());
    }

    // Final review I8: versions that differ are refused with a reason, never retried.
    #[tokio::test]
    async fn a_pc_without_a_version_is_refused_as_another_version() {
        let (a, b) = tokio::io::duplex(64 * 1024);
        let mac = tokio::spawn(async move { mac_handshake(b, &mac(), trusted).await.map(|_| ()) });
        let (read, mut write) = tokio::io::split(a);
        write.write_all(b"{\"id\":\"pc\",\"nonce\":\"00\"}\n").await.unwrap();
        let mut answer = String::new();
        BufReader::new(read).read_line(&mut answer).await.unwrap();
        assert_eq!(serde_json::from_str::<Refusal>(answer.trim()).unwrap(), Refusal::Version);
        assert!(mac.await.unwrap().is_err());
    }

    #[tokio::test]
    async fn a_mac_without_a_version_is_refused_as_another_version() {
        let (a, b) = tokio::io::duplex(64 * 1024);
        tokio::spawn(async move {
            let (read, mut write) = tokio::io::split(b);
            let mut hello = String::new();
            BufReader::new(read).read_line(&mut hello).await.unwrap();
            assert!(hello.contains(&format!("\"version\":{PROTOCOL}")), "{hello}");
            write.write_all(b"{\"nonce\":\"00\",\"id\":\"mac-1\",\"name\":\"Studio\"}\n").await.unwrap();
        });
        let pc = pc_handshake(a, "pc", TOKEN).await.map(|_| ());
        assert!(matches!(pc, Err(ShareHandshakeError::Refused(Refusal::Version))));
    }

    #[test]
    fn the_version_refusal_is_spelled_out() {
        assert_eq!(serde_json::to_string(&Refusal::Version).unwrap(), r#"{"refused":"version"}"#);
    }

    // Final review I8: an older Mac's answer has no id or name.
    #[test]
    fn an_answer_without_id_or_name_is_a_hello() {
        let reply: Reply = serde_json::from_str(r#"{"nonce":"00"}"#).unwrap();
        assert!(matches!(reply, Reply::Hello(MacHello { ref id, ref name, .. }) if id.is_empty() && name.is_empty()));
    }

    #[tokio::test]
    async fn a_busy_mac_tells_the_pc_who_is_connected() {
        let (pc, mac) = refused_with(Admit::Refuse(Refusal::Busy { by: "Desk PC".into() })).await;
        assert!(matches!(pc, Err(ShareHandshakeError::Refused(Refusal::Busy { by })) if by == "Desk PC"));
        assert_eq!(mac.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
    }

    #[tokio::test]
    async fn a_blocked_pc_is_told_it_is_blocked() {
        let (pc, mac) = refused_with(Admit::Refuse(Refusal::Blocked)).await;
        assert!(matches!(pc, Err(ShareHandshakeError::Refused(Refusal::Blocked))));
        assert!(mac.is_err());
    }

    #[tokio::test]
    async fn an_unknown_pc_is_refused() {
        let (a, b) = tokio::io::duplex(64 * 1024);
        let mac_task = tokio::spawn(async move { mac_handshake(b, &mac(), trusted).await.map(|_| ()) });
        let pc = pc_handshake(a, "someone else", TOKEN).await.map(|_| ());
        assert!(matches!(pc, Err(ShareHandshakeError::Refused(Refusal::Unknown))));
        assert!(mac_task.await.unwrap().is_err());
    }
}
