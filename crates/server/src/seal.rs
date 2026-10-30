//! Encryption at rest for Google refresh tokens.

use anyhow::{Context, Result, anyhow};
use chacha20poly1305::aead::{Aead, Generate, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};

const NONCE_LEN: usize = 24;

pub struct Sealer(XChaCha20Poly1305);

impl Sealer {
    /// `key` is the server's 32-byte token key.
    pub fn new(key: &[u8]) -> Result<Self> {
        XChaCha20Poly1305::new_from_slice(key)
            .map(Self)
            .map_err(|_| anyhow!("the token key must be exactly 32 bytes, got {}", key.len()))
    }

    /// Nonce followed by ciphertext.
    pub fn seal(&self, plaintext: &str) -> Result<Vec<u8>> {
        let nonce = XNonce::generate();
        let ciphertext = self
            .0
            .encrypt(&nonce, plaintext.as_bytes())
            .map_err(|_| anyhow!("encrypting a token"))?;
        Ok([nonce.as_slice(), &ciphertext].concat())
    }

    pub fn open(&self, sealed: &[u8]) -> Result<String> {
        let (nonce, ciphertext) = sealed
            .split_at_checked(NONCE_LEN)
            .context("sealed token is too short")?;
        let nonce = XNonce::try_from(nonce).map_err(|_| anyhow!("bad nonce length"))?;
        let plaintext = self
            .0
            .decrypt(&nonce, ciphertext)
            .map_err(|_| anyhow!("decrypting a token: wrong token key or corrupted data"))?;
        String::from_utf8(plaintext).context("decrypted token is not UTF-8")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_what_it_sealed_and_rejects_another_key() {
        let sealed = Sealer::new(&[7; 32]).unwrap().seal("1//refresh").unwrap();
        assert_eq!(
            Sealer::new(&[7; 32]).unwrap().open(&sealed).unwrap(),
            "1//refresh"
        );
        assert!(Sealer::new(&[8; 32]).unwrap().open(&sealed).is_err());
    }
}
