//! Pairing token and challenge-response, so the token is never sent after pairing.

use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::Sha256;

fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut buf);
    hex::encode(buf)
}

pub fn new_token() -> String {
    random_hex(32)
}

pub fn new_nonce() -> String {
    random_hex(16)
}

fn mac_for(token: &str, nonce: &str) -> Hmac<Sha256> {
    let mut mac = Hmac::<Sha256>::new_from_slice(token.as_bytes()).expect("HMAC accepts any key length");
    mac.update(nonce.as_bytes());
    mac
}

/// Hex HMAC-SHA256 with the token's bytes as key and the nonce's bytes as message.
pub fn sign(token: &str, nonce: &str) -> String {
    hex::encode(mac_for(token, nonce).finalize().into_bytes())
}

/// Constant-time check of a signature produced by [`sign`].
pub fn verify(token: &str, nonce: &str, mac: &str) -> bool {
    match hex::decode(mac) {
        Ok(bytes) => mac_for(token, nonce).verify_slice(&bytes).is_ok(),
        Err(_) => false,
    }
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
}
