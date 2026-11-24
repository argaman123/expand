//! Pairing with the Mac's six-digit code: SPAKE2, then key confirmation both ways, then the
//! token sealed under a key only the two ends of this exchange have.
//!
//! After the control link's hello and challenge (both saying [`PROTOCOL`]):
//!
//! 1. PC to Mac, `PairStart`. The Mac gets the code from its open pairing mode, or answers
//!    `PairResult { accepted: false }` straight away.
//! 2. Mac to PC, `PairKey { id, name, pake }`: SPAKE2 side B over Ed25519, keyed by the code, with
//!    the identities `sidescreen-pc:<pc id>` and `sidescreen-mac:<mac id>`.
//! 3. PC to Mac, `PairConfirm { pake, confirm }`: SPAKE2 side A, and the PC's key confirmation.
//! 4. Mac to PC, `PairResult { accepted, confirm, token }`: only once the PC's confirmation is
//!    right and pairing mode still admits it, the Mac's key confirmation and a new token sealed
//!    with ChaCha20-Poly1305.
//!
//! Keys: SPAKE2 gives a shared secret `K`, which already hashes in the code, both identities and
//! both SPAKE2 messages. The [`transcript`] hash `th` covers everything else either side keeps or
//! shows (both names, both ids, the challenge nonce, both messages, the version). HKDF-SHA256
//! with salt `th` and input `K` gives three independent keys: the PC's confirmation key, the
//! Mac's, and the token key. Each confirmation is HMAC-SHA256 over `th` with a label, under its
//! own key, so one side's confirmation can never be played back as the other's. The token key is
//! used exactly once (fresh `K` every exchange), so its nonce is fixed; `th` is the AAD.
//!
//! What an attacker gets: one passive recording holds nothing that tests a code offline (SPAKE2
//! messages are blinded; everything keyed comes from `K`). An active attacker gets one guess per
//! exchange, and every exchange the Mac runs counts against its pairing mode's attempts.

use super::LinkError;
use crate::protocol::PROTOCOL;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use spake2::{Ed25519Group, Identity, Password, Spake2};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

const PC_CONFIRM: &[u8] = b"sidescreen pairing: pc confirms";
const MAC_CONFIRM: &[u8] = b"sidescreen pairing: mac confirms";
const TOKEN_KEY: &[u8] = b"sidescreen pairing: token key";

fn pc_identity(pc_id: &str) -> Identity {
    Identity::new(format!("sidescreen-pc:{pc_id}").as_bytes())
}

fn mac_identity(mac_id: &str) -> Identity {
    Identity::new(format!("sidescreen-mac:{mac_id}").as_bytes())
}

/// Who took part and what was said: everything the two sides must agree on.
pub(crate) struct Transcript<'a> {
    pub pc_id: &'a str,
    pub pc_name: &'a str,
    pub mac_id: &'a str,
    pub mac_name: &'a str,
    pub nonce: &'a str,
    pub pc_pake: &'a [u8],
    pub mac_pake: &'a [u8],
}

/// SHA-256 over every field, each with its length in front, so no two transcripts hash alike
/// by moving bytes from one field to the next.
pub(crate) fn transcript(t: &Transcript) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"sidescreen pairing transcript");
    h.update(PROTOCOL.to_be_bytes());
    for field in [
        t.pc_id.as_bytes(),
        t.pc_name.as_bytes(),
        t.mac_id.as_bytes(),
        t.mac_name.as_bytes(),
        t.nonce.as_bytes(),
        t.pc_pake,
        t.mac_pake,
    ] {
        h.update((field.len() as u64).to_be_bytes());
        h.update(field);
    }
    h.finalize().into()
}

/// The keys one exchange derives from SPAKE2's shared secret, wiped when dropped.
#[derive(Zeroize, ZeroizeOnDrop)]
pub(crate) struct Keys {
    th: [u8; 32],
    pc_confirm: [u8; 32],
    mac_confirm: [u8; 32],
    token: [u8; 32],
}

impl Keys {
    pub(crate) fn derive(shared: &[u8], th: [u8; 32]) -> Keys {
        let hk = Hkdf::<Sha256>::new(Some(&th), shared);
        let mut keys = Keys { th, pc_confirm: [0; 32], mac_confirm: [0; 32], token: [0; 32] };
        // Expanded in place, so no copy of a key is left behind on the stack.
        for (label, key) in [(PC_CONFIRM, &mut keys.pc_confirm), (MAC_CONFIRM, &mut keys.mac_confirm), (TOKEN_KEY, &mut keys.token)] {
            hk.expand(label, key).expect("32 bytes is a valid HKDF-SHA256 length");
        }
        keys
    }

    fn mac(key: &[u8; 32], label: &[u8], th: &[u8; 32]) -> Hmac<Sha256> {
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key).expect("HMAC accepts any key length");
        mac.update(label);
        mac.update(th);
        mac
    }

    pub(crate) fn pc_confirmation(&self) -> String {
        hex::encode(Self::mac(&self.pc_confirm, PC_CONFIRM, &self.th).finalize().into_bytes())
    }

    pub(crate) fn mac_confirmation(&self) -> String {
        hex::encode(Self::mac(&self.mac_confirm, MAC_CONFIRM, &self.th).finalize().into_bytes())
    }

    /// Constant-time check of the PC's confirmation.
    pub(crate) fn pc_confirmed(&self, confirm: &str) -> bool {
        hex::decode(confirm).is_ok_and(|c| Self::mac(&self.pc_confirm, PC_CONFIRM, &self.th).verify_slice(&c).is_ok())
    }

    /// Constant-time check of the Mac's confirmation.
    pub(crate) fn mac_confirmed(&self, confirm: &str) -> bool {
        hex::decode(confirm).is_ok_and(|c| Self::mac(&self.mac_confirm, MAC_CONFIRM, &self.th).verify_slice(&c).is_ok())
    }

    fn cipher(&self) -> ChaCha20Poly1305 {
        ChaCha20Poly1305::new(Key::from_slice(&self.token))
    }

    /// The token, sealed (hex). The key seals this one message only, so a fixed nonce is safe.
    pub(crate) fn seal(&self, token: &str) -> String {
        let sealed = self
            .cipher()
            .encrypt(&Nonce::default(), Payload { msg: token.as_bytes(), aad: &self.th })
            .expect("sealing a short message cannot fail");
        hex::encode(sealed)
    }

    /// The token from [`Keys::seal`], when it opens under this exchange's key and is a token.
    pub(crate) fn open(&self, sealed: &str) -> Option<String> {
        let sealed = hex::decode(sealed).ok()?;
        let plain = Zeroizing::new(self.cipher().decrypt(&Nonce::default(), Payload { msg: &sealed, aad: &self.th }).ok()?);
        std::str::from_utf8(&plain).ok().filter(|t| is_token(t)).map(str::to_string)
    }
}

pub(crate) fn is_token(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// The PC's half, started once it knows who the Mac says it is.
pub(crate) struct PcSide {
    spake: Spake2<Ed25519Group>,
    pake: Vec<u8>,
}

impl PcSide {
    pub(crate) fn start(code: &str, pc_id: &str, mac_id: &str) -> PcSide {
        let (spake, pake) = Spake2::<Ed25519Group>::start_a(&Password::new(code.as_bytes()), &pc_identity(pc_id), &mac_identity(mac_id));
        PcSide { spake, pake }
    }

    pub(crate) fn message(&self) -> &[u8] {
        &self.pake
    }

    /// The keys, from the Mac's message. A message that is not a SPAKE2 side-B element is
    /// a refusal.
    pub(crate) fn finish(self, mac_pake: &[u8], fields: Transcript) -> Result<Keys, LinkError> {
        let shared = Zeroizing::new(self.spake.finish(mac_pake).map_err(|_| LinkError::PairRejected)?);
        Ok(Keys::derive(&shared, transcript(&fields)))
    }
}

/// The Mac's half.
pub(crate) struct MacSide {
    spake: Spake2<Ed25519Group>,
    pake: Vec<u8>,
}

impl MacSide {
    pub(crate) fn start(code: &str, pc_id: &str, mac_id: &str) -> MacSide {
        let (spake, pake) = Spake2::<Ed25519Group>::start_b(&Password::new(code.as_bytes()), &pc_identity(pc_id), &mac_identity(mac_id));
        MacSide { spake, pake }
    }

    pub(crate) fn message(&self) -> &[u8] {
        &self.pake
    }

    /// The keys, from the PC's message; `None` when it is not a SPAKE2 side-A element.
    pub(crate) fn finish(self, pc_pake: &[u8], fields: Transcript) -> Option<Keys> {
        let shared = Zeroizing::new(self.spake.finish(pc_pake).ok()?);
        Some(Keys::derive(&shared, transcript(&fields)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::new_token;

    fn fields<'a>(pc_pake: &'a [u8], mac_pake: &'a [u8]) -> Transcript<'a> {
        Transcript { pc_id: "pc-1", pc_name: "Desk", mac_id: "mac-1", mac_name: "MacBook", nonce: "00ff", pc_pake, mac_pake }
    }

    /// Both halves with these codes, as far as the keys.
    fn run(pc_code: &str, mac_code: &str) -> (Keys, Option<Keys>) {
        let pc = PcSide::start(pc_code, "pc-1", "mac-1");
        let mac = MacSide::start(mac_code, "pc-1", "mac-1");
        let (pc_msg, mac_msg) = (pc.message().to_vec(), mac.message().to_vec());
        let pc_keys = pc.finish(&mac_msg, fields(&pc_msg, &mac_msg)).unwrap();
        let mac_keys = mac.finish(&pc_msg, fields(&pc_msg, &mac_msg));
        (pc_keys, mac_keys)
    }

    #[test]
    fn the_same_code_gives_both_sides_the_same_keys() {
        let (pc, mac) = run("123456", "123456");
        let mac = mac.unwrap();
        assert!(mac.pc_confirmed(&pc.pc_confirmation()));
        assert!(pc.mac_confirmed(&mac.mac_confirmation()));
        let token = new_token();
        assert_eq!(pc.open(&mac.seal(&token)), Some(token));
    }

    #[test]
    fn another_code_gives_keys_that_confirm_nothing() {
        let (pc, mac) = run("123456", "123457");
        let mac = mac.unwrap();
        assert!(!mac.pc_confirmed(&pc.pc_confirmation()));
        assert!(!pc.mac_confirmed(&mac.mac_confirmation()));
        assert_eq!(pc.open(&mac.seal(&new_token())), None);
    }

    #[test]
    fn a_confirmation_does_not_work_in_the_other_direction() {
        let (pc, mac) = run("123456", "123456");
        let mac = mac.unwrap();
        assert!(!pc.mac_confirmed(&pc.pc_confirmation()));
        assert!(!mac.pc_confirmed(&mac.mac_confirmation()));
    }

    #[test]
    fn each_exchange_has_its_own_keys() {
        let (a, _) = run("123456", "123456");
        let (b, _) = run("123456", "123456");
        assert_ne!(a.pc_confirmation(), b.pc_confirmation());
    }

    #[test]
    fn a_different_transcript_gives_different_keys() {
        let shared = [7u8; 32];
        let (p, m) = ([1u8; 33], [2u8; 33]);
        let base = Keys::derive(&shared, transcript(&fields(&p, &m)));
        for changed in [
            Transcript { pc_name: "Other", ..fields(&p, &m) },
            Transcript { mac_name: "Other", ..fields(&p, &m) },
            Transcript { pc_id: "pc-2", ..fields(&p, &m) },
            Transcript { mac_id: "mac-2", ..fields(&p, &m) },
            Transcript { nonce: "00fe", ..fields(&p, &m) },
            Transcript { pc_pake: &m, mac_pake: &p, ..fields(&p, &m) },
            // Bytes moved from one field to the next.
            Transcript { pc_id: "pc-1D", pc_name: "esk", ..fields(&p, &m) },
        ] {
            let keys = Keys::derive(&shared, transcript(&changed));
            assert!(!keys.pc_confirmed(&base.pc_confirmation()));
            assert!(!base.mac_confirmed(&keys.mac_confirmation()));
            assert_eq!(base.open(&keys.seal(&new_token())), None);
        }
    }

    #[test]
    fn keys_are_wiped_when_dropped() {
        fn wiped_on_drop<T: ZeroizeOnDrop>() {}
        wiped_on_drop::<Keys>();
        let mut keys = Keys::derive(&[7u8; 32], [1u8; 32]);
        assert_ne!(keys.token, [0u8; 32]);
        assert_ne!(keys.pc_confirm, keys.mac_confirm);
        keys.zeroize();
        assert_eq!([keys.pc_confirm, keys.mac_confirm, keys.token], [[0u8; 32]; 3]);
    }

    #[test]
    fn a_sealed_token_that_was_changed_does_not_open() {
        let (pc, mac) = run("123456", "123456");
        let mac = mac.unwrap();
        let sealed = mac.seal(&new_token());
        let mut bytes = hex::decode(&sealed).unwrap();
        bytes[3] ^= 1;
        assert_eq!(pc.open(&hex::encode(bytes)), None);
        assert_eq!(pc.open(&sealed[..sealed.len() - 2]), None);
        assert_eq!(pc.open("not hex"), None);
    }

    #[test]
    fn only_a_token_is_taken_from_a_sealed_message() {
        let (pc, mac) = run("123456", "123456");
        assert_eq!(pc.open(&mac.unwrap().seal("short")), None);
    }

    #[test]
    fn the_sealed_token_is_not_the_token() {
        let (_, mac) = run("123456", "123456");
        let token = new_token();
        let sealed = mac.unwrap().seal(&token);
        assert!(!sealed.contains(&token));
        assert_eq!(sealed.len(), 2 * (64 + 16));
    }

    #[test]
    fn identities_name_both_devices() {
        // The PC started with another Mac's id than the Mac uses: the keys differ.
        let pc = PcSide::start("123456", "pc-1", "mac-2");
        let mac = MacSide::start("123456", "pc-1", "mac-1");
        let (pc_msg, mac_msg) = (pc.message().to_vec(), mac.message().to_vec());
        let pc_keys = pc.finish(&mac_msg, fields(&pc_msg, &mac_msg)).unwrap();
        let mac_keys = mac.finish(&pc_msg, fields(&pc_msg, &mac_msg)).unwrap();
        assert!(!mac_keys.pc_confirmed(&pc_keys.pc_confirmation()));
    }

    #[test]
    fn a_message_that_is_not_an_element_is_refused() {
        let pc = PcSide::start("123456", "pc-1", "mac-1");
        assert!(pc.finish(&[0x42; 10], fields(&[], &[])).is_err());
        let mac = MacSide::start("123456", "pc-1", "mac-1");
        // Its own side's message is not the other side's.
        let own = mac.message().to_vec();
        assert!(mac.finish(&own, fields(&[], &[])).is_none());
    }
}
