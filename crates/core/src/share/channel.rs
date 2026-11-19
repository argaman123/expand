//! The connection mouse and keyboard travel over: TCP, authenticated with the pairing token and
//! encrypted, so keystrokes never cross the network in the clear.
//!
//! Handshake: the PC sends `{"id", "nonce"}` as a line, the Mac answers `{"nonce"}` (it only
//! answers a PC it trusts). Each side derives one key per direction from the token and both
//! nonces (HKDF-SHA256), then the PC proves it has the token with a first encrypted frame.
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
const PROOF: &[u8] = b"sidescreen input";

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

/// PC side of the handshake.
pub async fn pc_handshake<S: AsyncRead + AsyncWrite>(stream: S, my_id: &str, token: &str) -> io::Result<Halves<S>> {
    let (read, mut write) = tokio::io::split(stream);
    let mut read = BufReader::new(read);
    let pc_nonce: [u8; 16] = rand::random();
    write_line(&mut write, &PcHello { id: my_id.to_string(), nonce: hex::encode(pc_nonce) }).await?;
    let answer: MacHello = read_line(&mut read).await?;
    let mac_nonce = hex::decode(&answer.nonce).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let (to_mac, to_pc) = derive(token, &pc_nonce, &mac_nonce);
    let mut sender = Sender { out: write, cipher: to_mac, counter: 0 };
    sender.send(&String::from_utf8_lossy(PROOF)).await?;
    // The Mac proves it holds the token too, before anything is captured and sent its way.
    let mut receiver = Receiver { inp: read, cipher: to_pc, counter: 0 };
    let proof: String = receiver.recv().await?;
    if proof.as_bytes() != PROOF {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "the Mac did not prove it is paired"));
    }
    Ok((receiver, sender))
}

/// Mac side of the handshake. `token_for` gives the token of a trusted PC by its id.
pub async fn mac_handshake<S: AsyncRead + AsyncWrite>(
    stream: S,
    token_for: impl Fn(&str) -> Option<String>,
) -> io::Result<Halves<S>> {
    let (read, mut write) = tokio::io::split(stream);
    let mut read = BufReader::new(read);
    let hello: PcHello = read_line(&mut read).await?;
    let token = token_for(&hello.id)
        .ok_or_else(|| io::Error::new(io::ErrorKind::PermissionDenied, "not the trusted PC"))?;
    let pc_nonce = hex::decode(&hello.nonce).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let mac_nonce: [u8; 16] = rand::random();
    write_line(&mut write, &MacHello { nonce: hex::encode(mac_nonce) }).await?;
    let (to_mac, to_pc) = derive(&token, &pc_nonce, &mac_nonce);
    let mut receiver = Receiver { inp: read, cipher: to_mac, counter: 0 };
    let proof: String = receiver.recv().await?;
    if proof.as_bytes() != PROOF {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "wrong proof"));
    }
    let mut sender = Sender { out: write, cipher: to_pc, counter: 0 };
    sender.send(&String::from_utf8_lossy(PROOF)).await?;
    Ok((receiver, sender))
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
    hk.expand(b"sidescreen input pc to mac", &mut to_mac).expect("32 bytes is a valid length");
    hk.expand(b"sidescreen input mac to pc", &mut to_pc).expect("32 bytes is a valid length");
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
}

#[derive(Serialize, Deserialize)]
struct MacHello {
    nonce: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn trusted(id: &str) -> Option<String> {
        (id == "pc").then(|| TOKEN.to_string())
    }

    async fn connected() -> (Halves<tokio::io::DuplexStream>, Halves<tokio::io::DuplexStream>) {
        let (a, b) = tokio::io::duplex(64 * 1024);
        let mac = tokio::spawn(async move { mac_handshake(b, trusted).await.unwrap() });
        let pc = pc_handshake(a, "pc", TOKEN).await.unwrap();
        (pc, mac.await.unwrap())
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
        let mac = tokio::spawn(async move { mac_handshake(b, trusted).await.map(|_| ()) });
        let _ = pc_handshake(a, "pc", &"b".repeat(64)).await;
        assert!(mac.await.unwrap().is_err());
    }

    #[tokio::test]
    async fn an_unknown_pc_is_turned_away() {
        let (a, b) = tokio::io::duplex(64 * 1024);
        let mac = tokio::spawn(async move { mac_handshake(b, trusted).await.map(|_| ()) });
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
            mac_handshake(bridge_b, trusted).await.unwrap()
        });
        let pc_stream = tokio::io::join(&mut a_read, &mut a_write);
        let (_, mut pc_out) = pc_handshake(pc_stream, "pc", TOKEN).await.unwrap();
        pc_out.send(&"my secret password").await.unwrap();
        let (mut mac_in, _) = mac.await.unwrap();
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
        tokio::spawn(async move { mac_handshake(b, |_| Some("b".repeat(64))).await.map(|_| ()) });
        assert!(pc_handshake(a, "pc", TOKEN).await.is_err());
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
        let mac = tokio::spawn(async move { mac_handshake(b, trusted).await.unwrap() });
        let (_, mut pc_out) = pc_handshake(a, "pc", TOKEN).await.unwrap();
        let (mut mac_in, _) = mac.await.unwrap();
        // Write a frame of the right length but garbage content straight onto the wire.
        let garbage = vec![0x5a; 40];
        pc_out.out.write_all(&(garbage.len() as u32).to_be_bytes()).await.unwrap();
        pc_out.out.write_all(&garbage).await.unwrap();
        assert!(mac_in.recv::<String>().await.is_err());
    }
}
