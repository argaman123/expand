//! Messages on the control link: one JSON object per line.

use crate::side::Side;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const MAX_LINE: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Message {
    Hello { name: String, id: String },
    Challenge { nonce: String },
    Auth { mac: String },
    AuthOk,
    AuthFailed,
    PairRequest { name: String, id: String, token: String },
    PairResult { accepted: bool },
    Start { width: u32, height: u32, fps: u32 },
    Ready,
    Failed { part: String, reason: String },
    SunshinePair { pin: String },
    SunshinePairResult { ok: bool },
    Stop,
    Stopped,
    Heartbeat,
    /// PC to Mac: which side of the PC the Mac sits on (where the virtual display goes, and
    /// which edge leads back to the PC).
    Arrangement { mac_side: Side },
    /// Mac to PC, before `Ready`: every address it can be reached at (home network, Tailscale,
    /// a cable link), so the PC can try them all next time.
    Addresses { list: Vec<String> },
}

fn invalid(msg: impl Into<String>) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, msg.into())
}

pub async fn write_message<W: AsyncWrite + Unpin>(w: &mut W, m: &Message) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(m).map_err(|e| invalid(e.to_string()))?;
    line.push(b'\n');
    w.write_all(&line).await?;
    w.flush().await
}

/// Reads one message. `Ok(None)` means the peer closed the stream cleanly.
pub async fn read_message<R: AsyncBufRead + Unpin>(r: &mut R) -> std::io::Result<Option<Message>> {
    let mut buf = Vec::new();
    let n = r.take(MAX_LINE as u64 + 1).read_until(b'\n', &mut buf).await?;
    if n == 0 {
        return Ok(None);
    }
    if buf.last() != Some(&b'\n') {
        return Err(invalid(if buf.len() > MAX_LINE {
            "message too long"
        } else {
            "stream ended in the middle of a message"
        }));
    }
    serde_json::from_slice(&buf).map(Some).map_err(|e| invalid(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::side::Side;
    use tokio::io::{AsyncWriteExt, BufReader};

    fn all_variants() -> Vec<Message> {
        vec![
            Message::Hello { name: "pc".into(), id: "1".into() },
            Message::Challenge { nonce: "aa".into() },
            Message::Auth { mac: "bb".into() },
            Message::AuthOk,
            Message::AuthFailed,
            Message::PairRequest { name: "pc".into(), id: "1".into(), token: "tok".into() },
            Message::PairResult { accepted: true },
            Message::Start { width: 2560, height: 1440, fps: 60 },
            Message::Ready,
            Message::Failed { part: "Sunshine".into(), reason: "already running".into() },
            Message::SunshinePair { pin: "1234".into() },
            Message::SunshinePairResult { ok: false },
            Message::Stop,
            Message::Stopped,
            Message::Heartbeat,
            Message::Arrangement { mac_side: Side::Right },
            Message::Addresses { list: vec!["192.168.1.50".into(), "100.64.0.2".into()] },
        ]
    }

    #[tokio::test]
    async fn every_variant_roundtrips() {
        let (a, b) = tokio::io::duplex(MAX_LINE * 2);
        let (_, mut w) = tokio::io::split(a);
        let (r, _) = tokio::io::split(b);
        let mut r = BufReader::new(r);
        for m in all_variants() {
            write_message(&mut w, &m).await.unwrap();
            assert_eq!(read_message(&mut r).await.unwrap(), Some(m));
        }
    }

    #[test]
    fn start_has_the_exact_wire_form() {
        let m = Message::Start { width: 2560, height: 1440, fps: 60 };
        assert_eq!(
            serde_json::to_string(&m).unwrap(),
            r#"{"type":"start","width":2560,"height":1440,"fps":60}"#
        );
    }

    #[test]
    fn arrangement_has_the_exact_wire_form() {
        let m = Message::Arrangement { mac_side: Side::Right };
        assert_eq!(serde_json::to_string(&m).unwrap(), r#"{"type":"arrangement","mac_side":"right"}"#);
    }

    async fn read_raw(bytes: &[u8]) -> std::io::Result<Option<Message>> {
        let (mut a, b) = tokio::io::duplex(MAX_LINE * 4);
        a.write_all(bytes).await.unwrap();
        drop(a);
        read_message(&mut BufReader::new(b)).await
    }

    #[tokio::test]
    async fn garbage_line_is_invalid_data() {
        let e = read_raw(b"hello there\n").await.unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::InvalidData);
    }

    #[tokio::test]
    async fn overlong_line_is_invalid_data() {
        let mut line = vec![b'x'; MAX_LINE + 10];
        line.push(b'\n');
        let e = read_raw(&line).await.unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::InvalidData);
    }

    #[tokio::test]
    async fn eof_is_none() {
        assert_eq!(read_raw(b"").await.unwrap(), None);
    }
}
