//! Encrypted keystore for Blink Engine credentials.
//!
//! Stores the signer private key + Polymarket API credentials in an
//! AES-256-GCM encrypted file, with the encryption key derived from a
//! passphrase via PBKDF2-HMAC-SHA256 (600 000 iterations).
//!
//! **Latency class: COLD PATH. Called once at startup only.**

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use anyhow::{bail, Context, Result};
use hmac::Hmac;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use zeroize::{Zeroize, Zeroizing};

/// Number of PBKDF2 iterations — OWASP recommends ≥600 000 for HMAC-SHA256.
const PBKDF2_ITERATIONS: u32 = 600_000;

/// AES-256-GCM nonce size in bytes.
const NONCE_SIZE: usize = 12;

/// PBKDF2 salt size in bytes.
const SALT_SIZE: usize = 32;

// ─── Public types ────────────────────────────────────────────────────────────

/// Decrypted credentials bundle.
#[derive(Serialize, Deserialize, Zeroize)]
#[zeroize(drop)]
pub struct KeystoreSecrets {
    pub signer_private_key: String,
    pub funder_address: String,
    pub api_key: String,
    pub api_secret: String,
    pub api_passphrase: String,
}

/// On-disk keystore format (JSON).
#[derive(Serialize, Deserialize)]
pub struct KeystoreFile {
    pub version: u8,
    pub address: String,
    pub crypto: CryptoParams,
    pub created_at: String,
}

#[derive(Serialize, Deserialize)]
pub struct CryptoParams {
    pub cipher: String,
    pub kdf: String,
    pub kdf_iterations: u32,
    pub salt: String,
    pub nonce: String,
    pub ciphertext: String,
}

// ─── Key generation ──────────────────────────────────────────────────────────

/// Generate a fresh secp256k1 keypair for Polymarket order signing.
/// Returns `(private_key_hex, ethereum_address)`.
pub fn generate_keypair() -> Result<(Zeroizing<String>, String)> {
    use k256::ecdsa::SigningKey;
    use sha3::{Digest, Keccak256};

    let signing_key = SigningKey::random(&mut rand::thread_rng());
    let verifying_key = signing_key.verifying_key();

    // Derive Ethereum address: keccak256(uncompressed_pubkey[1..]) → last 20 bytes
    let pubkey_bytes = verifying_key.to_encoded_point(false);
    let pubkey_uncompressed = &pubkey_bytes.as_bytes()[1..]; // skip 0x04 prefix
    let hash = Keccak256::digest(pubkey_uncompressed);
    let address = format!("0x{}", hex_encode(&hash[12..]));

    let private_key_hex = Zeroizing::new(hex_encode(signing_key.to_bytes().as_ref()));

    Ok((private_key_hex, address))
}

// ─── Encrypt / Decrypt ───────────────────────────────────────────────────────

/// Encrypt credentials and write to disk as a JSON keystore file.
pub fn encrypt_keystore(
    secrets: &KeystoreSecrets,
    passphrase: &str,
    path: &std::path::Path,
) -> Result<()> {
    let plaintext = serde_json::to_vec(secrets).context("serialize secrets")?;

    // Generate random salt and nonce
    let mut salt = [0u8; SALT_SIZE];
    let mut nonce_bytes = [0u8; NONCE_SIZE];
    rand::thread_rng().fill_bytes(&mut salt);
    rand::thread_rng().fill_bytes(&mut nonce_bytes);

    // Derive AES-256 key from passphrase via PBKDF2
    let mut derived_key = Zeroizing::new([0u8; 32]);
    pbkdf2_hmac_sha256(
        passphrase.as_bytes(),
        &salt,
        PBKDF2_ITERATIONS,
        &mut *derived_key,
    );

    // Encrypt with AES-256-GCM
    let cipher = Aes256Gcm::new_from_slice(&*derived_key)
        .map_err(|e| anyhow::anyhow!("AES key init: {e}"))?;
    let nonce = Nonce::<aes_gcm::aead::consts::U12>::from(nonce_bytes);
    let ciphertext = cipher
        .encrypt(&nonce, plaintext.as_ref())
        .map_err(|e| anyhow::anyhow!("AES-GCM encrypt: {e}"))?;

    // Build keystore file
    let keystore = KeystoreFile {
        version: 1,
        address: secrets.funder_address.clone(),
        crypto: CryptoParams {
            cipher: "aes-256-gcm".into(),
            kdf: "pbkdf2-hmac-sha256".into(),
            kdf_iterations: PBKDF2_ITERATIONS,
            salt: hex_encode(&salt),
            nonce: hex_encode(&nonce_bytes),
            ciphertext: hex_encode(&ciphertext),
        },
        created_at: chrono::Utc::now().to_rfc3339(),
    };

    // Atomic write: write to .tmp then rename
    let tmp_path = path.with_extension("tmp");
    let json = serde_json::to_string_pretty(&keystore).context("serialize keystore")?;
    std::fs::write(&tmp_path, json.as_bytes()).context("write keystore tmp")?;
    std::fs::rename(&tmp_path, path).context("rename keystore")?;

    tracing::info!(path = %path.display(), address = %keystore.address, "keystore saved");
    Ok(())
}

/// Decrypt a keystore file and return the secrets.
pub fn decrypt_keystore(path: &std::path::Path, passphrase: &str) -> Result<KeystoreSecrets> {
    let raw = std::fs::read(path).with_context(|| format!("read keystore: {}", path.display()))?;
    let keystore: KeystoreFile = serde_json::from_slice(&raw).context("parse keystore JSON")?;

    if keystore.version != 1 {
        bail!("unsupported keystore version: {}", keystore.version);
    }
    if keystore.crypto.cipher != "aes-256-gcm" {
        bail!("unsupported cipher: {}", keystore.crypto.cipher);
    }

    let salt = hex_decode(&keystore.crypto.salt).context("decode salt")?;
    let nonce_bytes = hex_decode(&keystore.crypto.nonce).context("decode nonce")?;
    let ciphertext = hex_decode(&keystore.crypto.ciphertext).context("decode ciphertext")?;

    if nonce_bytes.len() != NONCE_SIZE {
        bail!(
            "invalid nonce length: {} (expected {})",
            nonce_bytes.len(),
            NONCE_SIZE
        );
    }

    // Derive key from passphrase
    let mut derived_key = Zeroizing::new([0u8; 32]);
    pbkdf2_hmac_sha256(
        passphrase.as_bytes(),
        &salt,
        keystore.crypto.kdf_iterations,
        &mut *derived_key,
    );

    // Decrypt
    let cipher = Aes256Gcm::new_from_slice(&*derived_key)
        .map_err(|e| anyhow::anyhow!("AES key init: {e}"))?;
    let nonce_bytes: [u8; NONCE_SIZE] = nonce_bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("invalid nonce length"))?;
    let nonce = Nonce::<aes_gcm::aead::consts::U12>::from(nonce_bytes);
    let plaintext = cipher
        .decrypt(&nonce, ciphertext.as_ref())
        .map_err(|_| anyhow::anyhow!("decryption failed — wrong passphrase or corrupted file"))?;

    let secrets: KeystoreSecrets =
        serde_json::from_slice(&plaintext).context("parse decrypted secrets")?;

    tracing::info!(path = %path.display(), address = %keystore.address, "keystore decrypted");
    Ok(secrets)
}

// ─── PBKDF2 ──────────────────────────────────────────────────────────────────

/// PBKDF2-HMAC-SHA256 key derivation (RFC 8018).
fn pbkdf2_hmac_sha256(password: &[u8], salt: &[u8], iterations: u32, output: &mut [u8]) {
    use hmac::Mac;

    let key_len = output.len();
    let hash_len = 32; // SHA-256 output
    let blocks = key_len.div_ceil(hash_len);

    for block_idx in 1..=blocks {
        let mut u = {
            let mut mac =
                <Hmac<Sha256> as Mac>::new_from_slice(password).expect("HMAC key length is valid");
            mac.update(salt);
            mac.update(&(block_idx as u32).to_be_bytes());
            mac.finalize().into_bytes()
        };

        let mut result = u;

        for _ in 1..iterations {
            let mut mac =
                <Hmac<Sha256> as Mac>::new_from_slice(password).expect("HMAC key length is valid");
            mac.update(&u);
            u = mac.finalize().into_bytes();

            for (r, u_byte) in result.iter_mut().zip(u.iter()) {
                *r ^= u_byte;
            }
        }

        let start = (block_idx - 1) * hash_len;
        let end = (start + hash_len).min(key_len);
        output[start..end].copy_from_slice(&result[..end - start]);
    }
}

// ─── Hex helpers ─────────────────────────────────────────────────────────────

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hex_decode(hex: &str) -> Result<Vec<u8>> {
    if !hex.len().is_multiple_of(2) {
        bail!("odd hex length: {}", hex.len());
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).map_err(Into::into))
        .collect()
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_keypair_returns_valid_address() {
        let (key, addr) = generate_keypair().unwrap();
        assert_eq!(key.len(), 64, "private key should be 64 hex chars");
        assert!(addr.starts_with("0x"), "address should start with 0x");
        assert_eq!(addr.len(), 42, "address should be 42 chars");
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let dir = std::env::temp_dir().join("blink_keystore_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test_keystore.json");

        let secrets = KeystoreSecrets {
            signer_private_key: "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"
                .into(),
            funder_address: "0x1234567890abcdef1234567890abcdef12345678".into(),
            api_key: "test-api-key".into(),
            api_secret: "dGVzdC1zZWNyZXQ=".into(),
            api_passphrase: "test-pass".into(),
        };

        encrypt_keystore(&secrets, "hunter2", &path).unwrap();

        // Verify file exists and is valid JSON
        let raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(raw["version"], 1);
        assert_eq!(raw["crypto"]["cipher"], "aes-256-gcm");

        // Decrypt with correct passphrase
        let decrypted = decrypt_keystore(&path, "hunter2").unwrap();
        assert_eq!(decrypted.signer_private_key, secrets.signer_private_key);
        assert_eq!(decrypted.api_key, secrets.api_key);
        assert_eq!(decrypted.api_secret, secrets.api_secret);
        assert_eq!(decrypted.api_passphrase, secrets.api_passphrase);
        assert_eq!(decrypted.funder_address, secrets.funder_address);

        // Wrong passphrase should fail
        let err = decrypt_keystore(&path, "wrong-password");
        assert!(err.is_err(), "wrong passphrase should fail decryption");

        // Cleanup
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn pbkdf2_basic_derivation() {
        let mut out1 = [0u8; 32];
        let mut out2 = [0u8; 32];
        pbkdf2_hmac_sha256(b"password", b"salt", 1000, &mut out1);
        pbkdf2_hmac_sha256(b"password", b"salt", 1000, &mut out2);
        assert_eq!(out1, out2, "same inputs should produce same output");

        let mut out3 = [0u8; 32];
        pbkdf2_hmac_sha256(b"different", b"salt", 1000, &mut out3);
        assert_ne!(
            out1, out3,
            "different passwords should produce different output"
        );
    }
}
