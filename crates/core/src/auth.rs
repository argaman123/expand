//! Pairing token and challenge-response, so the token is never sent after pairing (pairing
//! itself, keyed by the code, is in `link::pairing`).

use hmac::{Hmac, Mac};
use rand::{Rng, RngCore};
use sha2::Sha256;

fn random_hex(bytes: usize) -> String {
    // The raw bytes of a token are wiped once encoded.
    let mut buf = zeroize::Zeroizing::new(vec![0u8; bytes]);
    rand::thread_rng().fill_bytes(&mut buf);
    hex::encode(&*buf)
}

pub fn new_token() -> String {
    random_hex(32)
}

pub fn new_nonce() -> String {
    random_hex(16)
}

/// A six-digit pairing code, uniformly random and zero-padded.
pub fn new_pair_code() -> String {
    format!("{:06}", rand::thread_rng().gen_range(0..1_000_000u32))
}

fn mac_for(key: &str, message: &str) -> Hmac<Sha256> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key.as_bytes()).expect("HMAC accepts any key length");
    mac.update(message.as_bytes());
    mac
}

/// Constant-time check of a hex HMAC-SHA256 over `message` keyed by `key`.
fn verify_mac(key: &str, message: &str, proof: &str) -> bool {
    match hex::decode(proof) {
        Ok(bytes) => mac_for(key, message).verify_slice(&bytes).is_ok(),
        Err(_) => false,
    }
}

/// Hex HMAC-SHA256 with the token's bytes as key and the nonce's bytes as message.
pub fn sign(token: &str, nonce: &str) -> String {
    hex::encode(mac_for(token, nonce).finalize().into_bytes())
}

/// Constant-time check of a signature produced by [`sign`].
pub fn verify(token: &str, nonce: &str, mac: &str) -> bool {
    verify_mac(token, nonce, mac)
}

/// Challenge response for connecting with a paired token. Keyed by the token, bound to the nonce.
pub fn connect_proof(token: &str, nonce: &str) -> String {
    hex::encode(mac_for(token, &format!("expand connect:{nonce}")).finalize().into_bytes())
}

/// Constant-time check of a proof produced by [`connect_proof`].
pub fn verify_connect_proof(token: &str, nonce: &str, proof: &str) -> bool {
    verify_mac(token, &format!("expand connect:{nonce}"), proof)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_verifies() {
        let (t, n) = (new_token(), new_nonce());
        assert!(verify(&t, &n, &sign(&t, &n)));
    }

    #[test]
    fn wrong_token_fails() {
        let n = new_nonce();
        assert!(!verify(&new_token(), &n, &sign(&new_token(), &n)));
    }

    #[test]
    fn wrong_nonce_fails() {
        let t = new_token();
        assert!(!verify(&t, &new_nonce(), &sign(&t, &new_nonce())));
    }

    #[test]
    fn truncated_or_malformed_mac_fails() {
        let (t, n) = (new_token(), new_nonce());
        let mac = sign(&t, &n);
        assert!(!verify(&t, &n, &mac[..mac.len() - 2]));
        assert!(!verify(&t, &n, "not hex"));
        assert!(!verify(&t, &n, ""));
    }

    #[test]
    fn tokens_are_random_and_64_hex_chars() {
        let (a, b) = (new_token(), new_token());
        assert_ne!(a, b);
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(new_nonce().len(), 32);
    }

    /// Vector from: printf '00' | openssl dgst -sha256 -hmac '00'
    #[test]
    fn matches_an_independent_hmac_sha256() {
        assert_eq!(sign("00", "00"), "236c4fb1ef79063c6622940c725d86c61d919fc098c379513106d1f1ded1ff60");
    }

    #[test]
    fn pair_codes_are_six_digits() {
        for _ in 0..1000 {
            let code = new_pair_code();
            assert_eq!(code.len(), 6);
            assert!(code.chars().all(|c| c.is_ascii_digit()));
        }
    }

    /// Vector from: printf 'expand connect:nonce-a' | openssl dgst -sha256 -hmac 'tok'
    #[test]
    fn connect_proof_matches_an_independent_hmac_sha256() {
        assert_eq!(connect_proof("tok", "nonce-a"), "293d4ea5ec479c57c5eddd5dc51f1443f3b54ca4bcf4c3622ed17677f0dd668d");
    }

    #[test]
    fn a_connect_proof_verifies_only_with_the_same_token_and_nonce() {
        let (token, nonce) = (new_token(), new_nonce());
        let proof = connect_proof(&token, &nonce);
        assert!(verify_connect_proof(&token, &nonce, &proof));
        assert!(!verify_connect_proof(&new_token(), &nonce, &proof));
        assert!(!verify_connect_proof(&token, &new_nonce(), &proof));
    }
}
