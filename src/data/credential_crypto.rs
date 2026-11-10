//! At-rest encryption for exchange API credentials.
//!
//! Credentials are serialized to JSON and encrypted with **AES-256-GCM**
//! before being written to `~/.config/finter/api_providers.json`. The 256-bit
//! key is generated on first use and stored in the OS keyring (macOS Keychain,
//! Linux Secret Service, Windows Credential Manager), then cached in process
//! memory for the lifetime of the running application.
//!
//! The key is never written to the configuration directory. Plaintext
//! credentials only exist in memory while FinTer is running; on disk the file
//! is always an authenticated ciphertext envelope.
//!
//! Envelope format (JSON):
//!
//! ```json
//! {
//!   "version": 1,
//!   "alg": "AES-256-GCM",
//!   "nonce": "<base64, 12 bytes>",
//!   "ciphertext": "<base64, ciphertext + 16-byte tag>"
//! }
//! ```
//!
//! The constant [`AAD`] is authenticated alongside the ciphertext so a blob
//! cannot be replayed in a different context.

use std::sync::Mutex;

use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, OsRng, Payload, rand_core::RngCore},
};
use anyhow::{Context, Result, anyhow, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

const KEYRING_SERVICE: &str = "finter";
const KEYRING_ACCOUNT: &str = "api-credentials-key";
const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 12;
const ALGORITHM: &str = "AES-256-GCM";
const ENVELOPE_VERSION: u32 = 1;

/// Additional authenticated data bound to every credential blob.
const AAD: &[u8] = b"finter.api-providers.v1";

#[derive(Serialize, Deserialize)]
struct Envelope {
    version: u32,
    alg: String,
    nonce: String,
    ciphertext: String,
}

impl Envelope {
    fn parse(bytes: &[u8]) -> Result<Self> {
        let envelope: Envelope =
            serde_json::from_slice(bytes).context("not an encrypted credential envelope")?;
        if envelope.version != ENVELOPE_VERSION {
            bail!(
                "unsupported credential envelope version {} (expected {})",
                envelope.version,
                ENVELOPE_VERSION
            );
        }
        if envelope.alg != ALGORITHM {
            bail!("unsupported credential algorithm {:?}", envelope.alg);
        }
        Ok(envelope)
    }
}

/// Cached key for the lifetime of the process. This is the only place the
/// plaintext key is kept: it is never persisted outside the OS keyring.
///
/// The mutex serialises first-use key creation so two threads cannot generate
/// competing keys and race to store them in the keyring.
static KEY_CACHE: Mutex<Option<[u8; KEY_LEN]>> = Mutex::new(None);

/// Returns `true` when `bytes` look like an encrypted credential envelope.
///
/// This is a cheap structural check and does not touch the keyring, so callers
/// can distinguish legacy plaintext files from encrypted ones without
/// prompting the OS for the key.
pub fn is_encrypted(bytes: &[u8]) -> bool {
    Envelope::parse(bytes).is_ok()
}

/// Load the credential key from the OS keyring, creating it on first use.
fn key() -> Result<[u8; KEY_LEN]> {
    let mut cached = KEY_CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(key) = *cached {
        return Ok(key);
    }

    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT)
        .context("OS keyring is unavailable; cannot access the credential key")?;

    let key = match entry.get_password() {
        Ok(encoded) => {
            let raw = B64
                .decode(encoded.trim())
                .context("credential key in the OS keyring is corrupt")?;
            raw.as_slice()
                .try_into()
                .map_err(|_| anyhow!("credential key in the OS keyring has the wrong length"))?
        }
        Err(keyring::Error::NoEntry) => {
            let mut generated = [0u8; KEY_LEN];
            OsRng.fill_bytes(&mut generated);
            entry
                .set_password(&B64.encode(generated))
                .context("failed to store the credential key in the OS keyring")?;
            generated
        }
        Err(e) => {
            return Err(anyhow!(
                "failed to read the credential key from the OS keyring: {e}"
            ));
        }
    };

    *cached = Some(key);
    Ok(key)
}

fn encrypt_with_key(key: &[u8; KEY_LEN], plaintext: &[u8]) -> Result<Vec<u8>> {
    let cipher =
        Aes256Gcm::new_from_slice(key).map_err(|_| anyhow!("invalid credential key length"))?;

    let mut nonce_bytes = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce_bytes);

    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce_bytes),
            Payload {
                msg: plaintext,
                aad: AAD,
            },
        )
        .map_err(|_| anyhow!("credential encryption failed"))?;

    let envelope = Envelope {
        version: ENVELOPE_VERSION,
        alg: ALGORITHM.to_string(),
        nonce: B64.encode(nonce_bytes),
        ciphertext: B64.encode(ciphertext),
    };

    serde_json::to_vec_pretty(&envelope).context("failed to serialize the encrypted envelope")
}

fn decrypt_with_key(key: &[u8; KEY_LEN], envelope: &Envelope) -> Result<Zeroizing<Vec<u8>>> {
    let nonce = B64
        .decode(envelope.nonce.trim())
        .context("encrypted credential nonce is not valid base64")?;
    if nonce.len() != NONCE_LEN {
        bail!("encrypted credential nonce has the wrong length");
    }

    let ciphertext = B64
        .decode(envelope.ciphertext.trim())
        .context("encrypted credential payload is not valid base64")?;

    let cipher =
        Aes256Gcm::new_from_slice(key).map_err(|_| anyhow!("invalid credential key length"))?;

    let plaintext = cipher
        .decrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: &ciphertext,
                aad: AAD,
            },
        )
        .map_err(|_| {
            anyhow!("credential decryption failed (wrong key, or the file was tampered with)")
        })?;

    Ok(Zeroizing::new(plaintext))
}

/// Encrypt `plaintext` using the key held in the OS keyring.
pub fn encrypt(plaintext: &[u8]) -> Result<Vec<u8>> {
    let key = key()?;
    encrypt_with_key(&key, plaintext)
}

/// Decrypt an encrypted credential envelope using the key held in the OS
/// keyring. The returned buffer is zeroed when dropped.
pub fn decrypt(bytes: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    let envelope = Envelope::parse(bytes)?;
    let key = key()?;
    decrypt_with_key(&key, &envelope)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key() -> [u8; KEY_LEN] {
        [7u8; KEY_LEN]
    }

    #[test]
    fn round_trip_preserves_plaintext() {
        let plaintext = br#"[{"exchange":"binance","api_key":"k","api_secret":"s"}]"#;
        let encrypted = encrypt_with_key(&test_key(), plaintext).expect("encrypt");
        assert!(is_encrypted(&encrypted));
        assert!(!encrypted.windows(8).any(|w| w == b"api_secret"));

        let decrypted =
            decrypt_with_key(&test_key(), &Envelope::parse(&encrypted).unwrap()).expect("decrypt");
        assert_eq!(&decrypted[..], plaintext);
    }

    #[test]
    fn random_nonce_produces_distinct_ciphertexts() {
        let plaintext = b"same input";
        let a = encrypt_with_key(&test_key(), plaintext).expect("encrypt a");
        let b = encrypt_with_key(&test_key(), plaintext).expect("encrypt b");
        assert_ne!(a, b);
    }

    #[test]
    fn tampering_is_rejected() {
        let plaintext = b"secret material";
        let encrypted = encrypt_with_key(&test_key(), plaintext).expect("encrypt");
        let mut envelope = Envelope::parse(&encrypted).unwrap();

        let mut raw = B64.decode(&envelope.ciphertext).unwrap();
        raw[0] ^= 0x01;
        envelope.ciphertext = B64.encode(raw);

        assert!(decrypt_with_key(&test_key(), &envelope).is_err());
    }

    #[test]
    fn wrong_key_is_rejected() {
        let encrypted = encrypt_with_key(&test_key(), b"secret material").expect("encrypt");
        let envelope = Envelope::parse(&encrypted).unwrap();
        let mut other = test_key();
        other[0] = 9;
        assert!(decrypt_with_key(&other, &envelope).is_err());
    }

    #[test]
    fn plaintext_is_not_mistaken_for_an_envelope() {
        assert!(!is_encrypted(b"[]"));
        assert!(!is_encrypted(br#"[{"exchange":"binance"}]"#));
    }
}
