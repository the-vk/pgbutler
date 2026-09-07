//! Symmetric encryption/decryption of arbitrary binary data using
//! AES-256-GCM, keyed by the master key stored via [`crate::config::keyring`].
//!
//! # Wire format
//!
//! `encrypt` returns `nonce (12 bytes) || ciphertext || tag (16 bytes)`, i.e.
//! the random nonce used for that message is prepended to the AEAD output
//! produced by the `aes-gcm` crate (whose [`Aead::encrypt`] already appends
//! the authentication tag after the ciphertext). `decrypt` expects exactly
//! that layout.

use aes_gcm::aead::{Aead, Generate, KeyInit, Nonce};
use aes_gcm::{Aes256Gcm, Key};
use openssl::sha::sha256;

use crate::config;

/// Number of bytes in a GCM nonce for `Aes256Gcm` (96 bits, the standard size).
const NONCE_LEN: usize = 12;

/// Encrypt `plaintext` with AES-256-GCM using the stored master key.
///
/// Accepts and returns raw binary data (`&[u8]`/`Vec<u8>`)
/// A fresh random nonce is generated for
/// every call and prepended to the returned ciphertext.
pub fn encrypt(plaintext: &[u8]) -> Result<Vec<u8>, String> {
    let cipher = cipher()?;
    let nonce = Nonce::<Aes256Gcm>::generate();
    let mut ciphertext = cipher
        .encrypt(&nonce, plaintext)
        .map_err(|e| format!("encryption failed: {e}"))?;

    let mut out = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    out.extend_from_slice(nonce.as_slice());
    out.append(&mut ciphertext);
    Ok(out)
}

/// Decrypt data previously produced by [`encrypt`], using the stored master
/// key. Returns an error if the master key is missing/wrong, the data is
/// too short to contain a nonce, or authentication fails (e.g. the data was
/// tampered with).
pub fn decrypt(data: &[u8]) -> Result<Vec<u8>, String> {
    if data.len() < NONCE_LEN {
        return Err("ciphertext too short to contain a nonce".to_string());
    }
    let (nonce_bytes, ciphertext) = data.split_at(NONCE_LEN);
    let nonce = Nonce::<Aes256Gcm>::try_from(nonce_bytes)
        .expect("split_at guarantees nonce_bytes.len() == NONCE_LEN");

    let cipher = cipher()?;
    cipher
        .decrypt(&nonce, ciphertext)
        .map_err(|e| format!("decryption failed: {e}"))
}

/// Build the AES-256-GCM cipher from the master key, creating and persisting
/// a new random master key on first use.
///
/// The master key stored in the keyring is an opaque string; it is hashed
/// with SHA-256 to derive the fixed 32-byte AES-256 key.
fn cipher() -> Result<Aes256Gcm, String> {
    let master_key = get_or_create_master_key()?;
    let key_bytes = sha256(master_key.as_bytes());
    let key = Key::<Aes256Gcm>::from(key_bytes);
    Ok(Aes256Gcm::new(&key))
}

fn get_or_create_master_key() -> Result<String, String> {
    if let Some(key) = config::read_master_key()? {
        return Ok(key);
    }
    let mut random_bytes = [0u8; 32];
    openssl::rand::rand_bytes(&mut random_bytes)
        .map_err(|e| format!("failed to generate master key: {e}"))?;
    let key = hex::encode(random_bytes);
    config::write_master_key(&key)?;
    Ok(key)
}

/// Minimal hex encoding helper.
mod hex {
    pub fn encode(bytes: [u8; 32]) -> String {
        use std::fmt::Write;
        let mut s = String::with_capacity(bytes.len() * 2);
        for b in bytes {
            write!(s, "{b:02x}").expect("writing to a String never fails");
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "touches the real macOS login Keychain (creates/reads the master key); run manually"]
    fn roundtrip_encrypt_decrypt() {
        let plaintext = b"super secret connection password";
        let ciphertext = encrypt(plaintext).expect("encrypt should succeed");
        assert_ne!(ciphertext, plaintext.to_vec());

        let decrypted = decrypt(&ciphertext).expect("decrypt should succeed");
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    #[ignore = "touches the real macOS login Keychain (creates/reads the master key); run manually"]
    fn tampered_ciphertext_fails_to_decrypt() {
        let mut ciphertext = encrypt(b"data").expect("encrypt should succeed");
        let last = ciphertext.len() - 1;
        ciphertext[last] ^= 0xFF;
        assert!(decrypt(&ciphertext).is_err());
    }
}
