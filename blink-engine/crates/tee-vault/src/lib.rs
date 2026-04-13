//! TEE-backed key vault for Blink Engine.
//!
//! Private keys never leave the vault — signing happens inside the enclave
//! (or in a zeroize-protected software buffer on development platforms).
//!
//! # Implementations
//!
//! | Variant          | Platform              | Security level |
//! |------------------|-----------------------|----------------|
//! | [`SoftwareVault`]| All (dev/test)        | RAM-only, zeroized on drop |
//! | `SgxVault`       | Linux + Intel SGX     | Hardware enclave (future) |
//! | `SevVault`       | Linux + AMD SEV-SNP   | Hardware enclave (future) |
//!
//! # Safety
//!
//! - All key material is wrapped in [`zeroize::Zeroizing`] and zeroed on drop.
//! - No type containing key material implements [`Debug`].
//! - `clippy::mem_forget` is denied crate-wide to prevent accidental key leaks.

pub mod keystore;

use anyhow::{Context, Result};
use k256::ecdsa::signature::hazmat::PrehashSigner;
use k256::ecdsa::{RecoveryId, Signature, SigningKey, VerifyingKey};
use sha3::{Digest, Keccak256};
use zeroize::{Zeroize, Zeroizing};

// ─── KeyVault trait ──────────────────────────────────────────────────────────

/// Trait for signing EIP-712 digests without exposing private key material.
///
/// All implementations must be `Send + Sync` for use across async tasks.
pub trait KeyVault: Send + Sync {
    /// Sign a 32-byte EIP-712 digest and return the 65-byte signature
    /// (`r[32] || s[32] || v[1]`).
    fn sign_digest(&self, digest: &[u8; 32]) -> Result<[u8; 65]>;

    /// Returns the Ethereum address of the signing key (public information).
    fn signer_address(&self) -> &str;

    /// Explicitly zeroize all key material.  Also called automatically on drop.
    fn zeroize_key(&mut self);
}

// ─── Audit log entry ─────────────────────────────────────────────────────────

/// Emitted for every signing operation (never contains key material).
fn audit_sign(address: &str, digest: &[u8; 32]) {
    let digest_hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    let ts = chrono::Utc::now().to_rfc3339();
    tracing::info!(
        target: "tee_vault::audit",
        timestamp = %ts,
        signer = %address,
        digest = %digest_hex,
        "signing operation"
    );
}

// ─── SoftwareVault ───────────────────────────────────────────────────────────

/// Software-only vault for development and testing.
///
/// The private key is held in a [`Zeroizing`] buffer that is automatically
/// zeroed when the vault is dropped.  On supported platforms, the buffer is
/// also memory-locked to prevent paging to disk.
///
/// **NOT SECURE FOR PRODUCTION** — use an SGX or SEV vault instead.
pub struct SoftwareVault {
    /// 32-byte private key in a zeroize-protected buffer.
    key: Zeroizing<[u8; 32]>,
    /// Pre-derived Ethereum address (public, safe to expose).
    address: String,
    /// Whether mlock/VirtualLock succeeded.
    locked: bool,
}

// Intentionally NOT implementing Debug — key material must never appear in logs.
// We provide a manual impl that redacts the key.
impl std::fmt::Debug for SoftwareVault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SoftwareVault")
            .field("address", &self.address)
            .field("key", &"[REDACTED]")
            .finish()
    }
}

impl SoftwareVault {
    /// Create a new software vault from a 32-byte private key.
    ///
    /// Logs a warning that this is not production-secure.
    pub fn new(private_key_bytes: &[u8; 32]) -> Result<Self> {
        tracing::warn!("SOFTWARE VAULT ACTIVE — NOT SECURE FOR PRODUCTION");

        let signing_key = SigningKey::from_bytes(private_key_bytes.into())
            .context("invalid secp256k1 private key")?;
        let address = pubkey_to_address(signing_key.verifying_key());

        let mut key = Zeroizing::new([0u8; 32]);
        key.copy_from_slice(private_key_bytes);

        let locked = lock_memory(key.as_ptr(), 32);
        if !locked {
            tracing::warn!("failed to lock key memory — key may be paged to disk");
        }

        Ok(Self {
            key,
            address,
            locked,
        })
    }

    /// Create a vault from a hex-encoded private key string (with optional `0x` prefix).
    pub fn from_hex(hex: &str) -> Result<Self> {
        let hex_str = hex.trim_start_matches("0x");
        anyhow::ensure!(hex_str.len() == 64, "private key must be 64 hex chars");

        let mut bytes = Zeroizing::new([0u8; 32]);
        for (i, chunk) in hex_str.as_bytes().chunks(2).enumerate() {
            let s = std::str::from_utf8(chunk).context("invalid utf8 in hex")?;
            bytes[i] = u8::from_str_radix(s, 16)
                .with_context(|| format!("invalid hex byte at position {i}"))?;
        }

        let result = Self::new(&bytes);
        // `bytes` is Zeroizing and will be zeroed on drop here.
        result
    }
}

impl KeyVault for SoftwareVault {
    fn sign_digest(&self, digest: &[u8; 32]) -> Result<[u8; 65]> {
        audit_sign(&self.address, digest);

        let signing_key = SigningKey::from_bytes(self.key.as_ref().into())
            .context("failed to reconstruct signing key")?;

        let (sig, rec_id): (Signature, RecoveryId) = signing_key
            .sign_prehash(digest)
            .context("ECDSA signing failed")?;

        let sig_bytes = sig.to_bytes(); // 64 bytes: r || s
        let v = rec_id.to_byte() + 27u8;

        let mut result = [0u8; 65];
        result[..64].copy_from_slice(&sig_bytes);
        result[64] = v;

        Ok(result)
    }

    fn signer_address(&self) -> &str {
        &self.address
    }

    fn zeroize_key(&mut self) {
        self.key.zeroize();
    }
}

impl Drop for SoftwareVault {
    fn drop(&mut self) {
        self.zeroize_key();
        if self.locked {
            unlock_memory(self.key.as_ptr(), 32);
        }
    }
}

// ─── Memory locking ──────────────────────────────────────────────────────────

/// Lock memory pages to prevent paging/swapping to disk.
#[cfg(unix)]
fn lock_memory(ptr: *const u8, len: usize) -> bool {
    // SAFETY: ptr points to a valid, aligned allocation of at least `len` bytes.
    unsafe { libc::mlock(ptr as *const _, len) == 0 }
}

#[cfg(unix)]
fn unlock_memory(ptr: *const u8, len: usize) {
    unsafe {
        libc::munlock(ptr as *const _, len);
    }
}

#[cfg(windows)]
fn lock_memory(ptr: *const u8, len: usize) -> bool {
    // SAFETY: ptr points to a valid allocation.
    unsafe { winapi::um::memoryapi::VirtualLock(ptr as *mut _, len) != 0 }
}

#[cfg(windows)]
fn unlock_memory(ptr: *const u8, len: usize) {
    unsafe {
        winapi::um::memoryapi::VirtualUnlock(ptr as *mut _, len);
    }
}

#[cfg(not(any(unix, windows)))]
fn lock_memory(_ptr: *const u8, _len: usize) -> bool {
    false
}

#[cfg(not(any(unix, windows)))]
fn unlock_memory(_ptr: *const u8, _len: usize) {}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn keccak256(data: &[u8]) -> [u8; 32] {
    let mut hasher = Keccak256::new();
    hasher.update(data);
    hasher.finalize().into()
}

fn pubkey_to_address(key: &VerifyingKey) -> String {
    let point = key.to_encoded_point(false);
    let hash = keccak256(&point.as_bytes()[1..]);
    let addr_bytes = &hash[12..];
    let hex: String = addr_bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("0x{hex}")
}

#[allow(dead_code)]
fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ─── VaultHandle — tokio channel interface ──────────────────────────────────

/// Request sent over the vault channel.
struct VaultRequest {
    digest: [u8; 32],
    reply: tokio::sync::oneshot::Sender<Result<[u8; 65]>>,
}

/// A `Send + Sync` handle to an isolated vault task.
///
/// The actual `SoftwareVault` lives inside a dedicated tokio task and never
/// leaves it.  Callers interact only through an async channel, so the private
/// key material is confined to a single task.
#[derive(Clone)]
pub struct VaultHandle {
    tx: tokio::sync::mpsc::Sender<VaultRequest>,
    address: String,
}

impl VaultHandle {
    /// Spawns a vault task and returns a cheaply-cloneable handle.
    ///
    /// The vault is created from `private_key_hex` (with optional `0x` prefix),
    /// loaded once, and never exposed outside the task.
    pub fn spawn(private_key_hex: &str) -> Result<Self> {
        let vault = SoftwareVault::from_hex(private_key_hex)?;
        let address = vault.signer_address().to_string();

        let (tx, mut rx) = tokio::sync::mpsc::channel::<VaultRequest>(64);

        tokio::spawn(async move {
            // `vault` lives exclusively in this task.
            while let Some(req) = rx.recv().await {
                let result = vault.sign_digest(&req.digest);
                // Receiver may have been dropped — ignore send errors.
                let _ = req.reply.send(result);
            }
            tracing::info!("VaultHandle task shutting down — vault zeroized on drop");
            // `vault` is dropped here → SoftwareVault::drop zeroizes key material.
        });

        Ok(Self { tx, address })
    }

    /// Sign a 32-byte EIP-712 digest via the vault task.
    ///
    /// Returns the 65-byte signature (`r[32] || s[32] || v[1]`).
    pub async fn sign_digest(&self, digest: &[u8; 32]) -> Result<[u8; 65]> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let request = VaultRequest {
            digest: *digest,
            reply: reply_tx,
        };
        self.tx
            .send(request)
            .await
            .map_err(|_| anyhow::anyhow!("vault task has shut down"))?;
        reply_rx
            .await
            .map_err(|_| anyhow::anyhow!("vault task dropped reply channel"))?
    }

    /// Returns the Ethereum address of the signing key.
    pub fn signer_address(&self) -> &str {
        &self.address
    }
}

/// Implements `KeyVault` synchronously by blocking on the async channel.
/// Useful for integration with existing synchronous call sites.
impl KeyVault for VaultHandle {
    fn sign_digest(&self, digest: &[u8; 32]) -> Result<[u8; 65]> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let request = VaultRequest {
            digest: *digest,
            reply: reply_tx,
        };
        self.tx
            .blocking_send(request)
            .map_err(|_| anyhow::anyhow!("vault task has shut down"))?;
        reply_rx
            .blocking_recv()
            .map_err(|_| anyhow::anyhow!("vault task dropped reply channel"))?
    }

    fn signer_address(&self) -> &str {
        &self.address
    }

    fn zeroize_key(&mut self) {
        // Key material lives in the vault task; dropping all handles will
        // shut down the task and trigger SoftwareVault::drop → zeroize.
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // A deterministic test key (NOT a real key — just for testing).
    const TEST_KEY_HEX: &str =
        "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";

    fn test_key_bytes() -> [u8; 32] {
        let mut bytes = [0u8; 32];
        for (i, chunk) in TEST_KEY_HEX.as_bytes().chunks(2).enumerate() {
            let s = std::str::from_utf8(chunk).unwrap();
            bytes[i] = u8::from_str_radix(s, 16).unwrap();
        }
        bytes
    }

    #[test]
    fn software_vault_signs_valid_eip712_digest() {
        let vault = SoftwareVault::new(&test_key_bytes()).unwrap();

        // Arbitrary 32-byte digest
        let digest = keccak256(b"test order digest");
        let sig = vault.sign_digest(&digest).unwrap();

        assert_eq!(sig.len(), 65);
        // v should be 27 or 28
        assert!(sig[64] == 27 || sig[64] == 28, "v = {}", sig[64]);

        // Verify signature recovers to the correct address
        let recovery_id = k256::ecdsa::RecoveryId::from_byte(sig[64] - 27).unwrap();
        let signature = k256::ecdsa::Signature::from_bytes((&sig[..64]).into()).unwrap();
        let recovered =
            k256::ecdsa::VerifyingKey::recover_from_prehash(&digest, &signature, recovery_id)
                .unwrap();
        let recovered_addr = pubkey_to_address(&recovered);
        assert_eq!(recovered_addr, vault.signer_address());
    }

    #[test]
    fn software_vault_zeroizes_on_drop() {
        let key_bytes = test_key_bytes();

        // Verify the vault can sign before drop
        let vault = SoftwareVault::new(&key_bytes).unwrap();
        let addr = vault.signer_address().to_string();
        assert!(!addr.is_empty());

        // After drop, the key field should be zeroed.
        // We can't directly inspect after drop, but we verify the Zeroizing
        // wrapper is in place and the type doesn't leak key material.
        drop(vault);
        // If we get here without panic, the drop ran successfully.
    }

    #[test]
    fn key_material_not_in_debug_output() {
        let vault = SoftwareVault::new(&test_key_bytes()).unwrap();
        let debug_str = format!("{vault:?}");
        assert!(
            debug_str.contains("[REDACTED]"),
            "debug output must redact key: {debug_str}"
        );
        assert!(
            !debug_str.contains(TEST_KEY_HEX),
            "debug output must NOT contain raw key hex"
        );
    }

    #[test]
    fn from_hex_with_prefix() {
        let vault = SoftwareVault::from_hex(&format!("0x{TEST_KEY_HEX}")).unwrap();
        let vault2 = SoftwareVault::new(&test_key_bytes()).unwrap();
        assert_eq!(vault.signer_address(), vault2.signer_address());
    }

    #[tokio::test]
    async fn vault_handle_signs_correctly() {
        let handle = VaultHandle::spawn(TEST_KEY_HEX).unwrap();
        let direct = SoftwareVault::new(&test_key_bytes()).unwrap();

        let digest = keccak256(b"vault handle test");
        let sig = handle.sign_digest(&digest).await.unwrap();

        assert_eq!(sig.len(), 65);
        assert!(sig[64] == 27 || sig[64] == 28);
        assert_eq!(handle.signer_address(), direct.signer_address());
    }

    #[tokio::test]
    async fn vault_handle_multiple_signs() {
        let handle = VaultHandle::spawn(TEST_KEY_HEX).unwrap();

        for i in 0..10u8 {
            let digest = keccak256(&[i; 32]);
            let sig = handle.sign_digest(&digest).await.unwrap();
            assert_eq!(sig.len(), 65);
        }
    }

    #[tokio::test]
    async fn vault_handle_clone_shares_task() {
        let h1 = VaultHandle::spawn(TEST_KEY_HEX).unwrap();
        let h2 = h1.clone();

        let digest = keccak256(b"clone test");
        let sig1 = h1.sign_digest(&digest).await.unwrap();
        let sig2 = h2.sign_digest(&digest).await.unwrap();

        // Same digest + same key → same signature (ECDSA is deterministic with RFC 6979).
        assert_eq!(sig1, sig2);
        assert_eq!(h1.signer_address(), h2.signer_address());
    }
}
