//! Encrypted key vault — AES-256-GCM with Argon2id KDF.
//!
//! Keys are encrypted at rest. The master key is derived from a
//! passphrase stored in macOS Keychain. In-memory key material
//! is zeroized after use.

pub mod store;

use aes_gcm::{
    aead::{Aead, KeyInit, OsRng},
    Aes256Gcm, Nonce,
};
use argon2::Argon2;
use rand::RngCore;
use zeroize::Zeroize;

const SALT_LEN: usize = 32;
const NONCE_LEN: usize = 12;

/// Derive a 256-bit key from a passphrase using Argon2id.
pub fn derive_key(passphrase: &[u8], salt: &[u8]) -> [u8; 32] {
    let mut key = [0u8; 32];
    Argon2::default()
        .hash_password_into(passphrase, salt, &mut key)
        .expect("Argon2 key derivation failed");
    key
}

/// Encrypt plaintext with AES-256-GCM.
/// Returns: salt (32) || nonce (12) || ciphertext
pub fn encrypt(plaintext: &[u8], passphrase: &[u8]) -> Vec<u8> {
    let mut salt = [0u8; SALT_LEN];
    OsRng.fill_bytes(&mut salt);

    let mut nonce_bytes = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce_bytes);

    let mut key = derive_key(passphrase, &salt);
    let cipher = Aes256Gcm::new_from_slice(&key).expect("key length");
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .expect("AES-GCM encryption failed");

    // Zeroize key material immediately
    key.zeroize();

    let mut result = Vec::with_capacity(SALT_LEN + NONCE_LEN + ciphertext.len());
    result.extend_from_slice(&salt);
    result.extend_from_slice(&nonce_bytes);
    result.extend_from_slice(&ciphertext);
    result
}

/// Decrypt ciphertext produced by `encrypt`.
pub fn decrypt(data: &[u8], passphrase: &[u8]) -> anyhow::Result<Vec<u8>> {
    if data.len() < SALT_LEN + NONCE_LEN + 16 {
        anyhow::bail!("Ciphertext too short");
    }

    let salt = &data[..SALT_LEN];
    let nonce_bytes = &data[SALT_LEN..SALT_LEN + NONCE_LEN];
    let ciphertext = &data[SALT_LEN + NONCE_LEN..];

    let mut key = derive_key(passphrase, salt);
    let cipher = Aes256Gcm::new_from_slice(&key).expect("key length");
    let nonce = Nonce::from_slice(nonce_bytes);

    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| anyhow::anyhow!("Decryption failed — wrong passphrase or corrupted data"))?;

    key.zeroize();
    Ok(plaintext)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let plaintext = b"AIzaSyDHD6xhNtU0AKSCSfdXOb8djdG7M-Qcs3w";
        let passphrase = b"test-master-key-do-not-use";

        let encrypted = encrypt(plaintext, passphrase);
        assert_ne!(encrypted, plaintext);

        let decrypted = decrypt(&encrypted, passphrase).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_wrong_passphrase_fails() {
        let plaintext = b"secret-api-key";
        let encrypted = encrypt(plaintext, b"correct");
        let result = decrypt(&encrypted, b"incorrect");
        assert!(result.is_err());
    }

    #[test]
    fn test_different_encryptions_differ() {
        let plaintext = b"same-key";
        let passphrase = b"same-pass";
        let e1 = encrypt(plaintext, passphrase);
        let e2 = encrypt(plaintext, passphrase);
        // Different salt + nonce → different ciphertext
        assert_ne!(e1, e2);
        // Both decrypt to same plaintext
        assert_eq!(decrypt(&e1, passphrase).unwrap(), plaintext);
        assert_eq!(decrypt(&e2, passphrase).unwrap(), plaintext);
    }
}
