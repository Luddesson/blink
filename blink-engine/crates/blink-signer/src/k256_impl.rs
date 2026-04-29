//! `EcdsaSigner` implementation backed by the pure-Rust `k256` crate.

use crate::eip712::keccak256;
use crate::{EcdsaSigner, SignatureRS, SignerError};

use k256::ecdsa::signature::hazmat::PrehashSigner;
use k256::ecdsa::{RecoveryId, Signature, SigningKey};

pub struct K256Signer {
    sk: SigningKey,
    address: [u8; 20],
}

impl K256Signer {
    pub fn from_bytes(pk: &[u8; 32]) -> Result<Self, SignerError> {
        let sk = SigningKey::from_bytes(pk.into())
            .map_err(|e| SignerError::InvalidKey(e.to_string()))?;
        let address = derive_address(&sk);
        Ok(Self { sk, address })
    }
}

fn derive_address(sk: &SigningKey) -> [u8; 20] {
    let vk = sk.verifying_key();
    // Uncompressed SEC1: 0x04 || X(32) || Y(32). Ethereum address = keccak256(X||Y)[12..].
    let pt = vk.to_encoded_point(false);
    let bytes = pt.as_bytes();
    debug_assert_eq!(bytes.len(), 65);
    debug_assert_eq!(bytes[0], 0x04);
    let h = keccak256(&bytes[1..]);
    let mut addr = [0u8; 20];
    addr.copy_from_slice(&h[12..]);
    addr
}

impl EcdsaSigner for K256Signer {
    #[inline]
    fn sign_prehash(&self, digest32: &[u8; 32]) -> SignatureRS {
        // `sign_prehash_recoverable` uses deterministic RFC6979 nonces; produces low-S.
        let (sig, rid): (Signature, RecoveryId) = PrehashSigner::sign_prehash(&self.sk, digest32)
            .expect("k256 sign_prehash_recoverable cannot fail for a 32-byte digest");

        // Defensive normalization: if for any reason s is in the high half,
        // flip it and toggle the y-parity bit of the recovery id.
        let (sig, rid) = match sig.normalize_s() {
            Some(s_norm) => (s_norm, RecoveryId::from_byte(rid.to_byte() ^ 1).unwrap()),
            None => (sig, rid),
        };

        let (r_fs, s_fs) = sig.split_bytes();
        let mut r = [0u8; 32];
        let mut s = [0u8; 32];
        r.copy_from_slice(r_fs.as_slice());
        s.copy_from_slice(s_fs.as_slice());
        SignatureRS {
            r,
            s,
            v: rid.to_byte(),
        }
    }

    #[inline]
    fn pubkey_address(&self) -> [u8; 20] {
        self.address
    }

    #[inline]
    fn impl_id(&self) -> &'static str {
        "k256"
    }
}
