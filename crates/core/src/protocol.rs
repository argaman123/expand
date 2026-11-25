//! Messages on the control link: one JSON object per line.

use crate::side::Side;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const MAX_LINE: usize = 64 * 1024;

/// Expand's protocol version, said in the control link's hello and challenge and in the
/// mouse and keyboard channel's hellos. 2: the sharing channel says its version and claims the
/// Mac only after the PC's proof. 3: pairing is a SPAKE2 exchange keyed by the code (the token
/// never crosses the network in the clear). A side that says no version is older than 3.
/// Versions that differ do not pair, and do not share the mouse and keyboard.
pub const PROTOCOL: u32 = 3;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Message {
    /// PC to Mac, first. `version` is [`PROTOCOL`] (0 from a PC older than 3).
    Hello {
        name: String,
        id: String,
        #[serde(default)]
        version: u32,
    },
    /// Mac to PC. `version` is [`PROTOCOL`] (0 from a Mac older than 3).
    Challenge {
        nonce: String,
        #[serde(default)]
        version: u32,
    },
    Auth { mac: String },
    AuthOk,
    AuthFailed,
    /// PC to Mac: pair with the code the Mac shows (see `link::pairing`).
    PairStart,
    /// Mac to PC: the Mac's SPAKE2 message (hex), and who the Mac is. Both are bound into the
    /// exchange, so a changed name or id is found out.
    PairKey { id: String, name: String, pake: String },
    /// PC to Mac: the PC's SPAKE2 message and its key confirmation (both hex).
    PairConfirm { pake: String, confirm: String },
    /// Mac to PC, last. When accepted: the Mac's key confirmation, and the new token sealed with
    /// ChaCha20-Poly1305 under a key only the two ends of this exchange have (both hex).
    PairResult {
        accepted: bool,
        #[serde(default)]
        confirm: String,
        #[serde(default)]
        token: String,
    },
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
            Message::Hello { name: "pc".into(), id: "1".into(), version: PROTOCOL },
            Message::Challenge { nonce: "aa".into(), version: PROTOCOL },
            Message::Auth { mac: "bb".into() },
            Message::AuthOk,
            Message::AuthFailed,
            Message::PairStart,
            Message::PairKey { id: "mac1".into(), name: "MacBook".into(), pake: "42ab".into() },
            Message::PairConfirm { pake: "41cd".into(), confirm: "ef".into() },
            Message::PairResult { accepted: true, confirm: "ab".into(), token: "cd".into() },
            Message::PairResult { accepted: false, confirm: String::new(), token: String::new() },
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

    #[test]
    fn hellos_from_before_version_3_read_as_version_0() {
        let hello: Message = serde_json::from_str(r#"{"type":"hello","name":"pc","id":"1"}"#).unwrap();
        assert_eq!(hello, Message::Hello { name: "pc".into(), id: "1".into(), version: 0 });
        let challenge: Message = serde_json::from_str(r#"{"type":"challenge","nonce":"aa"}"#).unwrap();
        assert_eq!(challenge, Message::Challenge { nonce: "aa".into(), version: 0 });
    }

    /// An older PC (or Mac) reads a challenge (or hello) that says a version, ignoring it.
    #[test]
    fn the_version_is_an_extra_field_older_sides_can_skip() {
        assert_eq!(
            serde_json::to_string(&Message::Challenge { nonce: "aa".into(), version: 3 }).unwrap(),
            r#"{"type":"challenge","nonce":"aa","version":3}"#
        );
    }

    /// Like 1.1 (no `deny_unknown_fields`), a side reads a hello or challenge with fields it does
    /// not know: a 1.1 peer still authenticates with a newer one, and is then told to update.
    #[test]
    fn unknown_fields_in_a_hello_or_challenge_are_skipped() {
        let hello: Message = serde_json::from_str(r#"{"type":"hello","name":"pc","id":"1","version":3,"later":true}"#).unwrap();
        assert_eq!(hello, Message::Hello { name: "pc".into(), id: "1".into(), version: 3 });
        let challenge: Message = serde_json::from_str(r#"{"type":"challenge","nonce":"aa","version":3,"later":[1]}"#).unwrap();
        assert_eq!(challenge, Message::Challenge { nonce: "aa".into(), version: 3 });
    }

    /// The old pairing request carried the token in the clear: it is no longer understood.
    #[test]
    fn the_old_pair_request_is_not_a_message() {
        let old = r#"{"type":"pair_request","name":"pc","id":"1","token":"aa","proof":"bb"}"#;
        assert!(serde_json::from_str::<Message>(old).is_err());
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
