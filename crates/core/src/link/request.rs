//! "Please connect": the Mac asks a PC it knows to bring its connection up. The PC listens on
//! [`REQUEST_PORT`]; the Mac sends one line proving it holds the pairing token and reads one line
//! back. Nothing is trusted from the line beyond that proof.

use crate::auth;
use serde::{Deserialize, Serialize};
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;

pub const REQUEST_PORT: u16 = 47802;

/// How long either side waits for the other to say its line.
const WAIT: Duration = Duration::from_secs(3);
/// A request is a few hundred bytes; anything longer is not one.
const MAX_LINE: u64 = 4096;
/// How many recent nonces the PC remembers, so a recorded request cannot be played again.
const REMEMBERED: usize = 256;

#[derive(Serialize, Deserialize)]
struct Request {
    mac_id: String,
    nonce: String,
    proof: String,
}

#[derive(Serialize, Deserialize)]
struct Reply {
    ok: bool,
}

/// Asks the PC at any of `addrs` to connect to this Mac. `Ok(false)` is a refusal: the PC does
/// not know this Mac, or does not accept its proof. An error means nobody answered.
pub async fn ask_to_connect(addrs: &[IpAddr], my_id: &str, token: &str) -> io::Result<bool> {
    let addrs: Vec<SocketAddr> = addrs.iter().map(|ip| SocketAddr::new(*ip, REQUEST_PORT)).collect();
    ask_at(&addrs, my_id, token).await
}

async fn ask_at(addrs: &[SocketAddr], my_id: &str, token: &str) -> io::Result<bool> {
    let Some((stream, _)) = super::client::first_reachable(addrs, WAIT).await else {
        return Err(io::Error::new(io::ErrorKind::NotConnected, "the PC is not reachable"));
    };
    let nonce = auth::new_nonce();
    let request = Request { mac_id: my_id.to_string(), proof: auth::connect_proof(token, &nonce), nonce };
    let talk = async {
        let mut stream = BufReader::new(stream);
        let mut line = serde_json::to_string(&request).map_err(io::Error::other)?;
        line.push_str("\n");
        stream.write_all(line.as_bytes()).await?;
        let mut reply = String::new();
        stream.take(MAX_LINE).read_line(&mut reply).await?;
        serde_json::from_str::<Reply>(&reply).map(|r| r.ok).map_err(io::Error::other)
    };
    timeout(WAIT, talk).await.unwrap_or_else(|_| Err(io::ErrorKind::TimedOut.into()))
}

/// The PC's side: answers requests on `bind` until the listener fails. `token_for` gives the
/// token of the Mac with that id (`None` for a Mac it does not know); `on_ok` is told the id of
/// each Mac that proved itself, and the PC should then connect to it.
pub async fn serve_requests(
    bind: SocketAddr,
    token_for: impl Fn(&str) -> Option<String> + Send + Sync + 'static,
    on_ok: impl Fn(String) + Send + Sync + 'static,
) -> io::Result<()> {
    serve_on(TcpListener::bind(bind).await?, token_for, on_ok).await
}

async fn serve_on(
    listener: TcpListener,
    token_for: impl Fn(&str) -> Option<String> + Send + Sync + 'static,
    on_ok: impl Fn(String) + Send + Sync + 'static,
) -> io::Result<()> {
    let token_for = Arc::new(token_for);
    let on_ok = Arc::new(on_ok);
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    loop {
        let (stream, _) = listener.accept().await?;
        let (token_for, on_ok, seen) = (token_for.clone(), on_ok.clone(), seen.clone());
        // Each caller in its own task, each within a short time: a silent one costs only itself.
        tokio::spawn(async move {
            let verdict = timeout(WAIT, answer(stream, &*token_for, &seen)).await;
            if let Ok(Ok(Some(mac_id))) = verdict {
                on_ok(mac_id);
            }
        });
    }
}

/// Reads one request and replies. Returns the Mac's id when the request was good.
async fn answer(
    stream: TcpStream,
    token_for: &impl Fn(&str) -> Option<String>,
    seen: &Mutex<Vec<String>>,
) -> io::Result<Option<String>> {
    let mut stream = BufReader::new(stream);
    let mut line = String::new();
    (&mut stream).take(MAX_LINE).read_line(&mut line).await?;
    let request: Option<Request> = serde_json::from_str(&line).ok();
    let good = request.filter(|r| {
        token_for(&r.mac_id).is_some_and(|token| auth::verify_connect_proof(&token, &r.nonce, &r.proof)) && {
            // Checked and remembered in one step, so two callers cannot both use one nonce.
            let mut seen = seen.lock().unwrap();
            let fresh = !seen.contains(&r.nonce);
            if fresh {
                seen.push(r.nonce.clone());
                if seen.len() > REMEMBERED {
                    seen.remove(0);
                }
            }
            fresh
        }
    });
    let mut reply = serde_json::to_string(&Reply { ok: good.is_some() }).map_err(io::Error::other)?;
    reply.push_str("\n");
    stream.write_all(reply.as_bytes()).await?;
    Ok(good.map(|r| r.mac_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;
    use tokio::sync::mpsc;

    const TOKEN: &str = "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1";

    /// A PC that knows one Mac, on a loopback port. Returns where it listens and the ids it was
    /// asked to connect to.
    async fn pc() -> (std::net::SocketAddr, mpsc::UnboundedReceiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let known = |id: &str| (id == "mac-1").then(|| TOKEN.to_string());
            let _ = serve_on(listener, known, move |id| {
                let _ = tx.send(id);
            })
            .await;
        });
        (addr, rx)
    }

    #[tokio::test]
    async fn a_valid_request_reaches_the_pc() {
        let (addr, mut asked) = pc().await;
        assert!(ask_at(&[addr], "mac-1", TOKEN).await.unwrap());
        assert_eq!(asked.recv().await.as_deref(), Some("mac-1"));
    }

    #[tokio::test]
    async fn a_request_with_a_wrong_proof_is_refused() {
        let (addr, mut asked) = pc().await;
        let other = "b2".repeat(32);
        assert!(!ask_at(&[addr], "mac-1", &other).await.unwrap());
        assert!(asked.try_recv().is_err());
    }

    #[tokio::test]
    async fn a_request_from_an_unknown_mac_is_refused() {
        let (addr, mut asked) = pc().await;
        assert!(!ask_at(&[addr], "mac-2", TOKEN).await.unwrap());
        assert!(asked.try_recv().is_err());
    }

    #[tokio::test]
    async fn a_silent_caller_does_not_hold_up_others() {
        let (addr, mut asked) = pc().await;
        let _silent = tokio::net::TcpStream::connect(addr).await.unwrap();
        let both = async {
            assert!(ask_at(&[addr], "mac-1", TOKEN).await.unwrap());
            asked.recv().await
        };
        let got = tokio::time::timeout(Duration::from_secs(1), both).await;
        assert_eq!(got.ok().flatten().as_deref(), Some("mac-1"));
    }

    #[tokio::test]
    async fn a_recorded_request_does_not_work_twice() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let (addr, mut asked) = pc().await;
        let nonce = crate::auth::new_nonce();
        let line = format!(
            "{}\n",
            serde_json::json!({"mac_id": "mac-1", "nonce": nonce, "proof": crate::auth::connect_proof(TOKEN, &nonce)})
        );
        let mut replies = Vec::new();
        for _ in 0..2 {
            let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
            stream.write_all(line.as_bytes()).await.unwrap();
            let mut reply = String::new();
            BufReader::new(stream).read_line(&mut reply).await.unwrap();
            replies.push(reply.contains("true"));
        }
        assert_eq!(replies, [true, false]);
        assert_eq!(asked.recv().await.as_deref(), Some("mac-1"));
        assert!(asked.try_recv().is_err());
    }
}
