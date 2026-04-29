//! Round-robin signer pool.
//!
//! Design choice: a `SignerPool` holds `Vec<Box<dyn EcdsaSigner>>`, one signer
//! per worker slot, and dispatches each `sign()` call to a slot chosen by an
//! atomic round-robin counter. Signatures themselves are synchronous (a handful
//! of µs); the pool exists purely for parallel throughput — multiple producer
//! threads can call `sign()` concurrently and each grabs a distinct, thread-safe
//! signer instance via `&dyn EcdsaSigner`.
//!
//! We do NOT use `thread_local!` at the pool level because the libsecp256k1
//! implementation already caches its heavy `Secp256k1<All>` context per thread
//! (see `secp256k1_impl`). The per-worker `Box<dyn EcdsaSigner>` keeps key
//! material separated so each slot can optionally hold a different key.

use std::sync::atomic::{AtomicUsize, Ordering};

use crate::{EcdsaSigner, K256Signer, Secp256k1Signer, SignatureRS, SignerError};

pub struct SignerPool {
    workers: Vec<Box<dyn EcdsaSigner>>,
    rr: AtomicUsize,
}

impl SignerPool {
    /// Build a pool of `K256Signer` workers. One worker per key.
    pub fn new_k256(keys: Vec<[u8; 32]>) -> Result<Self, SignerError> {
        if keys.is_empty() {
            return Err(SignerError::EmptyPool);
        }
        let mut workers: Vec<Box<dyn EcdsaSigner>> = Vec::with_capacity(keys.len());
        for k in &keys {
            workers.push(Box::new(K256Signer::from_bytes(k)?));
        }
        Ok(Self {
            workers,
            rr: AtomicUsize::new(0),
        })
    }

    /// Build a pool of `Secp256k1Signer` workers. One worker per key.
    pub fn new_secp(keys: Vec<[u8; 32]>) -> Result<Self, SignerError> {
        if keys.is_empty() {
            return Err(SignerError::EmptyPool);
        }
        let mut workers: Vec<Box<dyn EcdsaSigner>> = Vec::with_capacity(keys.len());
        for k in &keys {
            workers.push(Box::new(Secp256k1Signer::from_bytes(k)?));
        }
        Ok(Self {
            workers,
            rr: AtomicUsize::new(0),
        })
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.workers.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.workers.is_empty()
    }

    /// Round-robin sign. Thread-safe: each call advances the shared counter
    /// once and dispatches to `workers[idx % len]`.
    #[inline]
    pub fn sign(&self, digest32: &[u8; 32]) -> SignatureRS {
        let idx = self.rr.fetch_add(1, Ordering::Relaxed) % self.workers.len();
        self.workers[idx].sign_prehash(digest32)
    }

    /// Explicit worker-index sign. Useful for deterministic key selection
    /// (e.g. pick a signer by account index from a strategy).
    #[inline]
    pub fn sign_with(&self, worker_idx: usize, digest32: &[u8; 32]) -> SignatureRS {
        self.workers[worker_idx % self.workers.len()].sign_prehash(digest32)
    }

    /// Address of worker `idx`. Handy for CLOB order `maker` fields.
    #[inline]
    pub fn address(&self, worker_idx: usize) -> [u8; 20] {
        self.workers[worker_idx % self.workers.len()].pubkey_address()
    }

    #[inline]
    pub fn impl_id(&self) -> &'static str {
        self.workers[0].impl_id()
    }
}
