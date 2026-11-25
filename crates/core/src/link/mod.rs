//! The authenticated control link between the PC (client) and the Mac (server).

pub mod client;
mod pairing;
pub mod request;
pub mod server;

use crate::auth;
use crate::config::Peer;
use crate::protocol::{read_message, write_message, Message, PROTOCOL};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, BufReader, WriteHalf};
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;
use tokio::time::timeout;

pub const CONTROL_PORT: u16 = 47800;
pub const HEARTBEAT: Duration = Duration::from_secs(2);
pub const TIMEOUT: Duration = Duration::from_secs(10);
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug)]
pub enum LinkError {
    Unreachable,
    AuthFailed,
    PairRejected,
    NeedsCode,
    /// The other computer runs a version of Expand this one cannot pair with.
    Version,
    Closed,
    Io(std::io::Error),
}

impl std::fmt::Display for LinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LinkError::Unreachable => write!(f, "the other computer is not reachable"),
            LinkError::AuthFailed => write!(f, "the other computer no longer trusts this one"),
            LinkError::PairRejected => write!(f, "pairing was declined"),
            LinkError::NeedsCode => write!(f, "This Mac needs its pairing code"),
            LinkError::Version => write!(f, "the other computer runs another version of Expand; update it"),
            LinkError::Closed => write!(f, "the connection was lost"),
            LinkError::Io(e) => write!(f, "connection error: {e}"),
        }
    }
}

impl std::error::Error for LinkError {}

/// Decides whether an unknown PC may pair. Each pairing exchange asks [`PairGate::code`] once and
/// ends with exactly one [`PairGate::conclude`], even when the PC goes away halfway.
pub trait PairGate: Send + Sync {
    /// The code to pair with, while pairing mode is open; `None` refuses the PC at once.
    fn code(&self) -> Option<String>;
    /// Ends one exchange with `code`: `proved` when the PC proved it has the code. True when
    /// the PC may be paired, which must check, at this moment, that pairing mode is still open
    /// with this code (an exchange can outlast it). An exchange that ends unproved (a wrong code,
    /// a malformed or missing message, a PC that left) counts as one wrong attempt: each
    /// exchange lets the other side test one code.
    fn conclude(&self, code: &str, proved: bool) -> bool;
}

/// One pairing exchange on the Mac. Dropping it before [`Attempt::conclude`] counts as a miss.
struct Attempt<'a> {
    gate: &'a dyn PairGate,
    /// Wiped when the exchange ends.
    code: zeroize::Zeroizing<String>,
    open: bool,
}

impl Attempt<'_> {
    fn conclude(mut self, proved: bool) -> bool {
        self.open = false;
        // Never admitted without the proof, whatever the gate says.
        self.gate.conclude(&self.code, proved) && proved
    }
}

impl Drop for Attempt<'_> {
    fn drop(&mut self) {
        if self.open {
            self.gate.conclude(&self.code, false);
        }
    }
}

pub trait Stream: AsyncRead + AsyncWrite + Send + Unpin + 'static {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin + 'static> Stream for T {}

fn unexpected(what: &str) -> LinkError {
    LinkError::Io(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!("unexpected message during {what}"),
    ))
}

async fn read_within<S: Stream>(r: &mut BufReader<S>, limit: Duration) -> Result<Message, LinkError> {
    match timeout(limit, read_message(r)).await {
        Err(_) | Ok(Ok(None)) => Err(LinkError::Closed),
        Ok(Ok(Some(m))) => Ok(m),
        Ok(Err(e)) => Err(LinkError::Io(e)),
    }
}

async fn write<S: Stream>(w: &mut BufReader<S>, m: Message) -> Result<(), LinkError> {
    write_message(w, &m).await.map_err(|_| LinkError::Closed)
}

/// Mac side. Returns the stream ready for [`Connection::new`], plus the new peer if pairing just
/// happened (its `host` is left empty for the caller to fill in).
pub async fn server_handshake<S: Stream>(
    stream: S,
    known: Option<&Peer>,
    gate: &dyn PairGate,
    me: (&str, &str),
) -> Result<(BufReader<S>, Option<Peer>), LinkError> {
    let (s, new_peer, _) = server_handshake_by_id(stream, |_| known.cloned(), gate, me).await?;
    Ok((s, new_peer))
}

/// Like [`server_handshake`], but looks the PC up by the id in its hello.
pub(crate) async fn server_handshake_by_id<S: Stream>(
    stream: S,
    lookup: impl Fn(&str) -> Option<Peer>,
    gate: &dyn PairGate,
    me: (&str, &str),
) -> Result<(BufReader<S>, Option<Peer>, String), LinkError> {
    let mut s = BufReader::new(stream);
    let Message::Hello { name, id, version } = read_within(&mut s, TIMEOUT).await? else {
        return Err(unexpected("hello"));
    };
    let nonce = auth::new_nonce();
    write(&mut s, Message::Challenge { nonce: nonce.clone(), version: PROTOCOL }).await?;
    match read_within(&mut s, TIMEOUT).await? {
        // A known PC authenticates whatever its version: the sharing channel tells the user when
        // the two must be updated.
        Message::Auth { mac } => {
            let known = lookup(&id);
            let ok = known.is_some_and(|p| p.id == id && auth::verify(&p.token, &nonce, &mac));
            if ok {
                write(&mut s, Message::AuthOk).await?;
                Ok((s, None, id))
            } else {
                let _ = write(&mut s, Message::AuthFailed).await;
                Err(LinkError::AuthFailed)
            }
        }
        Message::PairStart => {
            if version != PROTOCOL {
                return refuse_pairing(&mut s, LinkError::Version).await;
            }
            let Some(code) = gate.code() else {
                return refuse_pairing(&mut s, LinkError::PairRejected).await;
            };
            // From here on, every way out counts this exchange against pairing mode.
            let attempt = Attempt { gate, code: zeroize::Zeroizing::new(code), open: true };
            let mac = pairing::MacSide::start(&attempt.code, &id, me.1);
            let mac_pake = mac.message().to_vec();
            write(&mut s, Message::PairKey { id: me.1.into(), name: me.0.into(), pake: hex::encode(&mac_pake) }).await?;
            let Message::PairConfirm { pake, confirm } = read_within(&mut s, TIMEOUT).await? else {
                return Err(unexpected("pairing"));
            };
            let pc_pake = hex::decode(&pake).unwrap_or_default();
            let fields = pairing::Transcript {
                pc_id: &id,
                pc_name: &name,
                mac_id: me.1,
                mac_name: me.0,
                nonce: &nonce,
                pc_pake: &pc_pake,
                mac_pake: &mac_pake,
            };
            let keys = mac.finish(&pc_pake, fields).filter(|keys| keys.pc_confirmed(&confirm));
            // Nothing keyed by this exchange goes out before the PC's confirmation is checked
            // and pairing mode has admitted it.
            let keys = match (attempt.conclude(keys.is_some()), keys) {
                (true, Some(keys)) => keys,
                _ => return refuse_pairing(&mut s, LinkError::PairRejected).await,
            };
            let token = auth::new_token();
            let result = Message::PairResult { accepted: true, confirm: keys.mac_confirmation(), token: keys.seal(&token) };
            // If the PC has gone by the time the answer is ready, nothing is paired.
            write(&mut s, result).await?;
            let pc_id = id.clone();
            Ok((s, Some(Peer { name, id, host: String::new(), token, sunshine_paired: false, addresses: Vec::new(), placement: Default::default(), stream: Default::default(), allowed: true }), pc_id))
        }
        _ => Err(unexpected("authentication")),
    }
}

/// Tells the PC its pairing is refused (best effort) and gives `why`.
async fn refuse_pairing<S: Stream, T>(s: &mut BufReader<S>, why: LinkError) -> Result<T, LinkError> {
    let _ = write(s, Message::PairResult { accepted: false, confirm: String::new(), token: String::new() }).await;
    Err(why)
}

/// PC side. With a token it authenticates; with no token it pairs, but only when it has the
/// Mac's `code` (otherwise [`LinkError::NeedsCode`]). The new peer carries the Mac's `name` and
/// `id`; its `host` is left empty for the caller to fill in.
pub async fn client_handshake<S: Stream>(
    stream: S,
    me_name: &str,
    me_id: &str,
    peer: Option<&Peer>,
    code: Option<&str>,
) -> Result<(BufReader<S>, Option<Peer>), LinkError> {
    let known = peer.filter(|p| !p.token.is_empty());
    if known.is_none() && code.is_none() {
        return Err(LinkError::NeedsCode);
    }
    let mut s = BufReader::new(stream);
    write(&mut s, Message::Hello { name: me_name.into(), id: me_id.into(), version: PROTOCOL }).await?;
    let Message::Challenge { nonce, version } = read_within(&mut s, TIMEOUT).await? else {
        return Err(unexpected("challenge"));
    };
    match (known, code) {
        (Some(p), _) => {
            write(&mut s, Message::Auth { mac: auth::sign(&p.token, &nonce) }).await?;
            match read_within(&mut s, TIMEOUT).await? {
                Message::AuthOk => Ok((s, None)),
                Message::AuthFailed => Err(LinkError::AuthFailed),
                _ => Err(unexpected("authentication")),
            }
        }
        (None, Some(code)) => {
            // An older Mac pairs the old way, with the token in the clear: never with this PC.
            if version != PROTOCOL {
                return Err(LinkError::Version);
            }
            write(&mut s, Message::PairStart).await?;
            let (mac_id, mac_name, mac_pake) = match read_within(&mut s, TIMEOUT).await? {
                Message::PairKey { id, name, pake } => (id, name, hex::decode(pake).map_err(|_| LinkError::PairRejected)?),
                Message::PairResult { .. } => return Err(LinkError::PairRejected),
                _ => return Err(unexpected("pairing")),
            };
            let pc = pairing::PcSide::start(code, me_id, &mac_id);
            let pc_pake = pc.message().to_vec();
            let fields = pairing::Transcript {
                pc_id: me_id,
                pc_name: me_name,
                mac_id: &mac_id,
                mac_name: &mac_name,
                nonce: &nonce,
                pc_pake: &pc_pake,
                mac_pake: &mac_pake,
            };
            let keys = pc.finish(&mac_pake, fields)?;
            write(&mut s, Message::PairConfirm { pake: hex::encode(&pc_pake), confirm: keys.pc_confirmation() }).await?;
            match read_within(&mut s, TIMEOUT).await? {
                // The Mac proves it ran the exchange with the same code, and the token opens
                // under this exchange's key; anything less is not the Mac the user paired with.
                Message::PairResult { accepted: true, confirm, token } if keys.mac_confirmed(&confirm) => {
                    let token = keys.open(&token).ok_or(LinkError::PairRejected)?;
                    Ok((
                        s,
                        Some(Peer {
                            name: mac_name,
                            id: mac_id,
                            host: String::new(),
                            token,
                            sunshine_paired: false,
                            addresses: Vec::new(),
                            placement: Default::default(),
                            stream: Default::default(),
                            allowed: true,
                        }),
                    ))
                }
                Message::PairResult { .. } => Err(LinkError::PairRejected),
                _ => Err(unexpected("pairing")),
            }
        }
        (None, None) => unreachable!("checked above"),
    }
}

type BoxStream = Box<dyn Stream>;

/// An authenticated link. Sends a heartbeat every [`HEARTBEAT`]; [`Connection::recv`] gives up
/// after [`TIMEOUT`] of silence.
pub struct Connection {
    incoming: mpsc::Receiver<Message>,
    writer: Arc<Mutex<WriteHalf<BoxStream>>>,
    tasks: [JoinHandle<()>; 2],
}

impl Connection {
    pub fn new<S: Stream>(stream: S) -> Connection {
        let (read_half, write_half) = tokio::io::split(Box::new(stream) as BoxStream);
        let writer = Arc::new(Mutex::new(write_half));
        let (tx, incoming) = mpsc::channel(32);

        let reader = tokio::spawn(async move {
            let mut r = BufReader::new(read_half);
            while let Ok(Some(m)) = read_message(&mut r).await {
                if tx.send(m).await.is_err() {
                    break;
                }
            }
        });

        let hb_writer = writer.clone();
        let heartbeat = tokio::spawn(async move {
            loop {
                tokio::time::sleep(HEARTBEAT).await;
                if write_message(&mut *hb_writer.lock().await, &Message::Heartbeat).await.is_err() {
                    break;
                }
            }
        });

        Connection { incoming, writer, tasks: [reader, heartbeat] }
    }

    pub async fn send(&mut self, m: Message) -> Result<(), LinkError> {
        write_message(&mut *self.writer.lock().await, &m).await.map_err(|_| LinkError::Closed)
    }

    /// The next message that is not a heartbeat. Safe to use inside `select!`.
    pub async fn recv(&mut self) -> Result<Message, LinkError> {
        loop {
            match timeout(TIMEOUT, self.incoming.recv()).await {
                Ok(Some(Message::Heartbeat)) => continue,
                Ok(Some(m)) => return Ok(m),
                Ok(None) | Err(_) => return Err(LinkError::Closed),
            }
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}
