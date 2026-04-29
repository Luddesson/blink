//! EIP-712 struct-hash and domain-separator helpers.
//!
//! All hashing uses `Keccak256` (NOT SHA-3-256) per the Ethereum/EIP-712 spec.

use sha3::{Digest, Keccak256};

/// Canonical EIP-712 domain type string.
pub const DOMAIN_TYPE_STRING: &[u8] =
    b"EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)";

/// EIP-712 domain parameters. Configurable — do not hard-code venue constants here.
#[derive(Clone, Debug)]
pub struct Eip712Domain {
    pub name: String,
    pub version: String,
    pub chain_id: u64,
    pub verifying_contract: [u8; 20],
}

impl Eip712Domain {
    /// Polymarket CTF Exchange domain on Polygon mainnet (chain_id = 137).
    ///
    /// Matches `engine/src/order_signer.rs` `DOMAIN_TYPE_STRING` + name/version.
    /// Pass the verifying-contract address explicitly so callers don't bake it in.
    pub fn polymarket_ctf(verifying_contract: [u8; 20]) -> Self {
        Self {
            name: "Polymarket CTF Exchange".to_string(),
            version: "1".to_string(),
            chain_id: 137,
            verifying_contract,
        }
    }
}

#[inline]
pub fn keccak256(input: &[u8]) -> [u8; 32] {
    let mut h = Keccak256::new();
    h.update(input);
    h.finalize().into()
}

/// Standard EIP-712 struct hash: `keccak256(typeHash || encodedData)`.
///
/// `encoded_data` must already be ABI-encoded per the caller's type layout
/// (32-byte slots, left-padded primitives, hashes of dynamic fields).
#[inline]
pub fn struct_hash(type_hash: &[u8; 32], encoded_data: &[u8]) -> [u8; 32] {
    let mut h = Keccak256::new();
    h.update(type_hash);
    h.update(encoded_data);
    h.finalize().into()
}

/// EIP-712 domain separator = `keccak256(typeHash || nameHash || versionHash || chainId || verifyingContract)`.
pub fn domain_separator(d: &Eip712Domain) -> [u8; 32] {
    let type_hash = keccak256(DOMAIN_TYPE_STRING);
    let name_hash = keccak256(d.name.as_bytes());
    let version_hash = keccak256(d.version.as_bytes());

    let mut chain_id_b32 = [0u8; 32];
    chain_id_b32[24..].copy_from_slice(&d.chain_id.to_be_bytes());

    let mut addr_b32 = [0u8; 32];
    addr_b32[12..].copy_from_slice(&d.verifying_contract);

    let mut enc = [0u8; 32 * 5];
    enc[0..32].copy_from_slice(&type_hash);
    enc[32..64].copy_from_slice(&name_hash);
    enc[64..96].copy_from_slice(&version_hash);
    enc[96..128].copy_from_slice(&chain_id_b32);
    enc[128..160].copy_from_slice(&addr_b32);
    keccak256(&enc)
}

/// Final EIP-712 typed-data digest = `keccak256(0x19 || 0x01 || domainSep || structHash)`.
#[inline]
pub fn typed_data_digest(domain_sep: &[u8; 32], struct_hash_: &[u8; 32]) -> [u8; 32] {
    let mut buf = [0u8; 2 + 32 + 32];
    buf[0] = 0x19;
    buf[1] = 0x01;
    buf[2..34].copy_from_slice(domain_sep);
    buf[34..66].copy_from_slice(struct_hash_);
    keccak256(&buf)
}
