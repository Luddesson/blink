//! `EcdsaSigner` implementation backed by `libsecp256k1` (via the `secp256k1` crate).
//!
//! Each thread that calls `sign_prehash` gets its own pre-warmed `Secp256k1<All>`
//! context. libsecp256k1 contexts are expensive to create (randomization tables)
//! but cheap to reuse, so we cache one per thread.

use std::cell::RefCell;

use secp256k1::{ecdsa::RecoverableSignature, All, Message, PublicKey, Secp256k1, SecretKey};

use crate::eip712::keccak256;
use crate::{EcdsaSigner, SignatureRS, SignerError};

thread_local! {
    static CTX: RefCell<Secp256k1<All>> = RefCell::new(Secp256k1::new());
}

pub struct Secp256k1Signer {
    sk: SecretKey,
    address: [u8; 20],
}

impl Secp256k1Signer {
    pub fn from_bytes(pk: &[u8; 32]) -> Result<Self, SignerError> {
        let sk = SecretKey::from_slice(pk).map_err(|e| SignerError::InvalidKey(e.to_string()))?;
        let address = CTX.with(|ctx| {
            let ctx = ctx.borrow();
            let vk = PublicKey::from_secret_key(&ctx, &sk);
            let uncompressed = vk.serialize_uncompressed(); // 65 bytes, 0x04 || X || Y
            let h = keccak256(&uncompressed[1..]);
            let mut addr = [0u8; 20];
            addr.copy_from_slice(&h[12..]);
            addr
        });
        Ok(Self { sk, address })
    }
}

impl EcdsaSigner for Secp256k1Signer {
    #[inline]
    fn sign_prehash(&self, digest32: &[u8; 32]) -> SignatureRS {
        let msg = Message::from_digest(*digest32);
        let sig: RecoverableSignature = CTX.with(|ctx| {
            let ctx = ctx.borrow();
            // libsecp256k1 uses RFC6979 deterministic nonces and produces
            // low-S signatures by default.
            ctx.sign_ecdsa_recoverable(&msg, &self.sk)
        });
        let (rid, compact) = sig.serialize_compact();
        let mut r = [0u8; 32];
        let mut s = [0u8; 32];
        r.copy_from_slice(&compact[..32]);
        s.copy_from_slice(&compact[32..]);
        SignatureRS {
            r,
            s,
            v: i32::from(rid) as u8,
        }
    }

    #[inline]
    fn pubkey_address(&self) -> [u8; 20] {
        self.address
    }

    #[inline]
    fn impl_id(&self) -> &'static str {
        "secp256k1"
    }
}
