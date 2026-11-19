//! The authenticated control link between the PC (client) and the Mac (server).

pub mod client;
pub mod server;

use crate::auth;
use crate::config::Peer;
use crate::protocol::{read_message, write_message, Message};
use async_trait::async_trait;
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
/// How long the person at the Mac has to answer the trust dialog.
pub const PROMPT_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug)]
pub enum LinkError {
    Unreachable,
    AuthFailed,
    PairRejected,
    Closed,
    Io(std::io::Error),
}

impl std::fmt::Display for LinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LinkError::Unreachable => write!(f, "the other computer is not reachable"),
            LinkError::AuthFailed => write!(f, "the other computer no longer trusts this one"),
            LinkError::PairRejected => write!(f, "pairing was declined"),
            LinkError::Closed => write!(f, "the connection was lost"),
            LinkError::Io(e) => write!(f, "connection error: {e}"),
        }
    }
}

impl std::error::Error for LinkError {}

/// Decides whether an unknown PC may pair. The app shows a dialog.
#[async_trait]
pub trait TrustPrompt: Send + Sync {
    async fn ask(&self, name: &str, id: &str) -> bool;
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

fn is_token(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// Mac side. Returns the stream ready for [`Connection::new`], plus the new peer if pairing just
/// happened (its `host` is left empty for the caller to fill in).
pub async fn server_handshake<S: Stream>(
    stream: S,
    known: Option<&Peer>,
    prompt: &dyn TrustPrompt,
) -> Result<(BufReader<S>, Option<Peer>), LinkError> {
    let mut s = BufReader::new(stream);
    let Message::Hello { name: _, id } = read_within(&mut s, TIMEOUT).await? else {
        return Err(unexpected("hello"));
    };
    let nonce = auth::new_nonce();
    write(&mut s, Message::Challenge { nonce: nonce.clone() }).await?;
    match read_within(&mut s, TIMEOUT).await? {
        Message::Auth { mac } => {
            let ok = known.is_some_and(|p| p.id == id && auth::verify(&p.token, &nonce, &mac));
            if ok {
                write(&mut s, Message::AuthOk).await?;
                Ok((s, None))
            } else {
                let _ = write(&mut s, Message::AuthFailed).await;
                Err(LinkError::AuthFailed)
            }
        }
        Message::PairRequest { name, id, token } => {
            let accepted = is_token(&token)
                && timeout(PROMPT_TIMEOUT, prompt.ask(&name, &id)).await.unwrap_or(false);
            // If the PC has gone by the time the answer is ready, nothing was paired.
            let told = write(&mut s, Message::PairResult { accepted }).await;
            if !accepted {
                return Err(LinkError::PairRejected);
            }
            told?;
            Ok((s, Some(Peer { name, id, host: String::new(), token, sunshine_paired: false, addresses: Vec::new() })))
        }
        _ => Err(unexpected("authentication")),
    }
}

/// PC side. With no peer (or an empty token) it asks to pair and returns the new peer
/// (`name`, `id` and `host` are left empty for the caller to fill in).
pub async fn client_handshake<S: Stream>(
    stream: S,
    me_name: &str,
    me_id: &str,
    peer: Option<&Peer>,
) -> Result<(BufReader<S>, Option<Peer>), LinkError> {
    let mut s = BufReader::new(stream);
    write(&mut s, Message::Hello { name: me_name.into(), id: me_id.into() }).await?;
    let Message::Challenge { nonce } = read_within(&mut s, TIMEOUT).await? else {
        return Err(unexpected("challenge"));
    };
    match peer.filter(|p| !p.token.is_empty()) {
        Some(p) => {
            write(&mut s, Message::Auth { mac: auth::sign(&p.token, &nonce) }).await?;
            match read_within(&mut s, TIMEOUT).await? {
                Message::AuthOk => Ok((s, None)),
                Message::AuthFailed => Err(LinkError::AuthFailed),
                _ => Err(unexpected("authentication")),
            }
        }
        None => {
            let token = auth::new_token();
            let request = Message::PairRequest { name: me_name.into(), id: me_id.into(), token: token.clone() };
            write(&mut s, request).await?;
            match read_within(&mut s, PROMPT_TIMEOUT + TIMEOUT).await? {
                Message::PairResult { accepted: true } => Ok((
                    s,
                    Some(Peer {
                        name: String::new(),
                        id: String::new(),
                        host: String::new(),
                        token,
                        sunshine_paired: false,
                        addresses: Vec::new(),
                    }),
                )),
                Message::PairResult { accepted: false } => Err(LinkError::PairRejected),
                _ => Err(unexpected("pairing")),
            }
        }
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
