//! `blink-signer` — EIP-712 signer pool for Polymarket CTF orders.
//!
//! Public surface is intentionally narrow:
//!
//! - [`EcdsaSigner`] trait + [`SignatureRS`] type — the abstraction over any
//!   ECDSA-secp256k1 backend.
//! - [`SignerPool`] — a round-robin pool of workers.
//! - [`eip712`] — struct-hash / domain-separator helpers.
//!
//! The concrete backends ([`K256Signer`], [`Secp256k1Signer`]) are exposed only
//! so callers can pick one explicitly; crypto crate types are not re-exported.
//!
//! Both backends produce IDENTICAL `(r, s)` bytes for the same key + digest
//! (RFC6979 deterministic nonces + low-S normalization). See tests.

#![forbid(unsafe_code)]

pub mod eip712;
mod k256_impl;
mod pool;
mod secp256k1_impl;

pub use k256_impl::K256Signer;
pub use pool::SignerPool;
pub use secp256k1_impl::Secp256k1Signer;

/// Compact ECDSA signature in the form emitted by both backends after low-S
/// normalization. `v` is the recovery id (0 or 1); callers that need the
/// Ethereum `v = 27 + rid` convention should add 27 at the edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SignatureRS {
    pub r: [u8; 32],
    pub s: [u8; 32],
    pub v: u8,
}

impl SignatureRS {
    /// Concatenated `r || s || v` (65 bytes), matching the encoding
    /// Polymarket's CLOB accepts.
    pub fn to_bytes65(&self) -> [u8; 65] {
        let mut out = [0u8; 65];
        out[0..32].copy_from_slice(&self.r);
        out[32..64].copy_from_slice(&self.s);
        out[64] = self.v;
        out
    }
}

pub trait EcdsaSigner: Send + Sync {
    /// Sign a 32-byte prehash digest. Output is low-S normalized.
    fn sign_prehash(&self, digest32: &[u8; 32]) -> SignatureRS;

    /// Ethereum address = `keccak256(pubkey[1..])[12..]`.
    fn pubkey_address(&self) -> [u8; 20];

    /// Human-readable backend name (`"k256"` or `"secp256k1"`).
    fn impl_id(&self) -> &'static str;
}

#[derive(Debug, thiserror::Error)]
pub enum SignerError {
    #[error("invalid private key: {0}")]
    InvalidKey(String),
    #[error("signer pool must contain at least one key")]
    EmptyPool,
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::eip712::{domain_separator, keccak256, typed_data_digest, Eip712Domain};
    use super::*;

    fn hex32(s: &str) -> [u8; 32] {
        let bytes = hex::decode(s).expect("hex");
        let mut out = [0u8; 32];
        out.copy_from_slice(&bytes);
        out
    }

    fn hex20(s: &str) -> [u8; 20] {
        let bytes = hex::decode(s).expect("hex");
        let mut out = [0u8; 20];
        out.copy_from_slice(&bytes);
        out
    }

    /// Private key = 1 (canonical Ethereum test vector) → address
    /// 0x7E5F4552091A69125d5DfCb7b8C2659029395Bdf.
    #[test]
    fn known_address_vector_k256() {
        let mut pk = [0u8; 32];
        pk[31] = 1;
        let signer = K256Signer::from_bytes(&pk).unwrap();
        let expected = hex20("7e5f4552091a69125d5dfcb7b8c2659029395bdf");
        assert_eq!(signer.pubkey_address(), expected);
    }

    #[test]
    fn known_address_vector_secp() {
        let mut pk = [0u8; 32];
        pk[31] = 1;
        let signer = Secp256k1Signer::from_bytes(&pk).unwrap();
        let expected = hex20("7e5f4552091a69125d5dfcb7b8c2659029395bdf");
        assert_eq!(signer.pubkey_address(), expected);
    }

    /// Core correctness: the two backends must produce byte-identical (r, s, v)
    /// for the same key + digest. RFC6979 + low-S guarantees this.
    #[test]
    fn cross_impl_equality() {
        let keys = [
            hex32("4646464646464646464646464646464646464646464646464646464646464646"),
            hex32("1111111111111111111111111111111111111111111111111111111111111111"),
            hex32("deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef"),
        ];
        let digests = [
            [0u8; 32],
            keccak256(b"hello polymarket"),
            keccak256(b""),
            keccak256(&[0xaa; 256]),
        ];

        for key in &keys {
            let k = K256Signer::from_bytes(key).unwrap();
            let s = Secp256k1Signer::from_bytes(key).unwrap();
            assert_eq!(k.pubkey_address(), s.pubkey_address());
            for d in &digests {
                let a = k.sign_prehash(d);
                let b = s.sign_prehash(d);
                assert_eq!(a.r, b.r, "r mismatch");
                assert_eq!(a.s, b.s, "s mismatch");
                assert_eq!(a.v, b.v, "v mismatch");
                // Low-S invariant: s < n/2 → high bit of s never set.
                assert!(a.s[0] < 0x80, "high-S output from k256");
                assert!(b.s[0] < 0x80, "high-S output from secp256k1");
            }
        }
    }

    #[test]
    fn domain_separator_stable_vector() {
        let dom = Eip712Domain {
            name: "Polymarket CTF Exchange".to_string(),
            version: "1".to_string(),
            chain_id: 137,
            verifying_contract: hex20("4bfb41d5b3570defd03c39a9a4d8de6bd8b8982e"),
        };
        let ds = domain_separator(&dom);
        let ds2 = domain_separator(&dom.clone());
        assert_eq!(ds, ds2);
        assert!(ds.iter().any(|b| *b != 0));

        let struct_h = keccak256(b"test");
        let digest = typed_data_digest(&ds, &struct_h);
        assert_eq!(digest.len(), 32);
        let digest2 = typed_data_digest(&ds, &struct_h);
        assert_eq!(digest, digest2);

        // Changing any field changes the separator.
        let mut d2 = dom.clone();
        d2.chain_id = 1;
        assert_ne!(domain_separator(&d2), ds);
    }

    #[test]
    fn pool_round_robin_distribution() {
        let keys: Vec<[u8; 32]> = (1u8..=4)
            .map(|i| {
                let mut k = [0u8; 32];
                k[31] = i;
                k
            })
            .collect();
        let pool = SignerPool::new_k256(keys).unwrap();
        assert_eq!(pool.len(), 4);

        let digest = keccak256(b"rr");
        // 4 distinct worker addresses must be reachable.
        let mut addrs = std::collections::HashSet::<[u8; 20]>::new();
        for i in 0..4 {
            addrs.insert(pool.address(i));
        }
        assert_eq!(addrs.len(), 4);

        // Round-robin wraps cleanly. fetch_add returns old value, so the
        // worker index used by call c is (c % N). To see the same worker
        // twice, the counter must advance by a multiple of N between calls.
        // `before` uses counter 0; 3 intermediate calls consume 1..=3; then
        // `after` uses counter 4 ≡ 0 (mod 4).
        let before = pool.sign(&digest);
        for _ in 0..3 {
            let _ = pool.sign(&digest);
        }
        let after = pool.sign(&digest);
        assert_eq!(before, after, "rr counter wraps cleanly every pool.len()");

        // Also assert 4 consecutive calls hit 4 distinct workers: since each
        // worker has a distinct key, the 4 signatures must be pairwise
        // different.
        let sigs: Vec<_> = (0..4).map(|_| pool.sign(&digest)).collect();
        for i in 0..4 {
            for j in (i + 1)..4 {
                assert_ne!(sigs[i], sigs[j], "workers {i} and {j} collided");
            }
        }
    }

    #[test]
    fn pool_secp_and_k256_agree() {
        let key = hex32("1111111111111111111111111111111111111111111111111111111111111111");
        let pk = SignerPool::new_k256(vec![key]).unwrap();
        let ps = SignerPool::new_secp(vec![key]).unwrap();
        let d = keccak256(b"pool agreement");
        assert_eq!(pk.sign(&d), ps.sign(&d));
        assert_eq!(pk.address(0), ps.address(0));
        assert_eq!(pk.impl_id(), "k256");
        assert_eq!(ps.impl_id(), "secp256k1");
    }

    #[test]
    fn empty_pool_rejected() {
        assert!(SignerPool::new_k256(vec![]).is_err());
        assert!(SignerPool::new_secp(vec![]).is_err());
    }

    #[test]
    fn sig_to_bytes65_layout() {
        let mut pk = [0u8; 32];
        pk[31] = 2;
        let s = K256Signer::from_bytes(&pk).unwrap();
        let sig = s.sign_prehash(&keccak256(b"layout"));
        let bytes = sig.to_bytes65();
        assert_eq!(&bytes[0..32], &sig.r);
        assert_eq!(&bytes[32..64], &sig.s);
        assert_eq!(bytes[64], sig.v);
    }
}
