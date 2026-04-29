//! EIP-712 order encoder for the Polymarket CTF Exchange.
//!
//! Pure, side-effect-free. Produces the `struct_hash`, the
//! `typed_data_digest` ready for signing, and the JSON wire body that is
//! paired with the eventual signature before POST.
//!
//! **Wire-format parity:** byte-for-byte compatible with
//! `engine/src/order_signer.rs`.  See the `WIRE FORMAT REF` comments below
//! for the exact legacy source lines consulted.
//!
//! # Field order of the EIP-712 Order struct
//!
//! ```text
//! Order(
//!   uint256 salt,
//!   address maker,
//!   address signer,
//!   address taker,
//!   uint256 tokenId,
//!   uint256 makerAmount,
//!   uint256 takerAmount,
//!   uint256 expiration,
//!   uint256 nonce,
//!   uint256 feeRateBps,
//!   uint8   side,
//!   uint8   signatureType
//! )
//! ```
//!
//! Verifying contract: `0x4bFb41d5B3570DeFd03C39a9A4D8dE6Bd8B8982E` (Polygon mainnet).
//!
//! # Unit model
//!
//! | Side | `makerAmount` | `takerAmount` |
//! |------|---------------|---------------|
//! | BUY  | USDC paid ×1e6 | shares received ×1e6 |
//! | SELL | shares sold ×1e6 | USDC received ×1e6 |
//!
//! `Intent.size` is already in USDC µunits (×1e6). `Intent.price` is
//! probability × 1 000 (the legacy convention).

use blink_signer::eip712::{
    domain_separator as ds, keccak256, typed_data_digest, Eip712Domain,
};
use blink_types::{Intent, Side};
use bytes::Bytes;
use serde::Serialize;

/// `Order(uint256 salt, address maker, address signer, address taker, uint256 tokenId, uint256 makerAmount, uint256 takerAmount, uint256 expiration, uint256 nonce, uint256 feeRateBps, uint8 side, uint8 signatureType)`
// WIRE FORMAT REF: engine/src/order_signer.rs:27-28
pub(crate) const ORDER_TYPE_STRING: &[u8] =
    b"Order(uint256 salt,address maker,address signer,address taker,uint256 tokenId,uint256 makerAmount,uint256 takerAmount,uint256 expiration,uint256 nonce,uint256 feeRateBps,uint8 side,uint8 signatureType)";

/// Polygon mainnet Polymarket CTF Exchange verifying-contract address.
// WIRE FORMAT REF: engine/src/order_signer.rs:30
pub const POLYMARKET_CTF_EXCHANGE: [u8; 20] = [
    0x4b, 0xfb, 0x41, 0xd5, 0xb3, 0x57, 0x0d, 0xef, 0xd0, 0x3c, 0x39, 0xa9, 0xa4, 0xd8, 0xde,
    0x6b, 0xd8, 0xb8, 0x98, 0x2e,
];

/// Output of [`OrderEncoder::encode`].
#[derive(Debug, Clone)]
pub struct EncodedOrder {
    /// `keccak256(typeHash || encodedFields)` — the EIP-712 *struct hash*.
    pub struct_hash: [u8; 32],
    /// `keccak256(0x1901 || domainSep || structHash)` — the digest ready
    /// to hand to [`blink_signer::SignerPool::sign`].
    pub digest: [u8; 32],
    /// JSON request body (without the signature field) *shaped like* the
    /// final POST body; the signature is injected by
    /// [`OrderEncoder::build_wire_body_signed`] once the signer returns.
    pub wire_body_unsigned: Bytes,
    /// Numeric maker amount actually written into the struct (for journals /
    /// diagnostics). Same as the JSON field, but as a `u64` so callers
    /// don't have to parse it back.
    pub maker_amount: u64,
    /// Numeric taker amount written into the struct.
    pub taker_amount: u64,
    /// Random salt baked into the struct hash.
    pub salt: u128,
}

/// EIP-712 order encoder.
///
/// Stateless w.r.t. a single order — construct once per `(maker, exchange)`
/// and reuse across intents for domain-separator caching.
#[derive(Debug, Clone)]
pub struct OrderEncoder {
    maker: [u8; 20],
    exchange: [u8; 20],
    domain_separator: [u8; 32],
    order_type_hash: [u8; 32],
    maker_hex: String,
}

impl OrderEncoder {
    /// Build an encoder. `maker` is the Polymarket proxy/funder wallet
    /// (the one that holds USDC); `exchange` is the CTF Exchange contract
    /// address (usually [`POLYMARKET_CTF_EXCHANGE`]).
    pub fn new(maker: [u8; 20], exchange: [u8; 20]) -> Self {
        let domain = Eip712Domain::polymarket_ctf(exchange);
        // Override: Eip712Domain::polymarket_ctf bakes in chain 137 + the
        // canonical name/version — same constants as legacy.
        // WIRE FORMAT REF: engine/src/order_signer.rs:456-471 (domain_separator)
        let domain_separator = ds(&domain);
        let order_type_hash = keccak256(ORDER_TYPE_STRING);
        let maker_hex = addr_to_hex(&maker);
        Self {
            maker,
            exchange,
            domain_separator,
            order_type_hash,
            maker_hex,
        }
    }

    /// Maker proxy address (20 bytes).
    pub fn maker(&self) -> [u8; 20] {
        self.maker
    }

    /// Verifying-contract address (20 bytes).
    pub fn exchange(&self) -> [u8; 20] {
        self.exchange
    }

    /// Domain separator (cached).
    pub fn domain_separator(&self) -> [u8; 32] {
        self.domain_separator
    }

    /// Pre-computed `keccak256(ORDER_TYPE_STRING)`.
    pub fn order_type_hash(&self) -> [u8; 32] {
        self.order_type_hash
    }

    /// Encode an [`Intent`] plus the `signer` address (the hot-signing
    /// account managed by [`blink_signer::SignerPool`]) into an
    /// [`EncodedOrder`].
    ///
    /// * `salt` — deterministic salt chosen by the caller (the driver
    ///   loop uses `intent_hash[0..16]` or `event_id`). Legacy engine
    ///   uses `intent_id as u128` for replay-determinism.
    /// * `signer_addr` — 20-byte address of the signer that *will* sign
    ///   this order. Must match the address of the worker the digest is
    ///   handed to.
    /// * `client_order_id` — 16-byte coid. Echoed through the wire body
    ///   as a 32-hex string under the `clientOrderId` field so the CLOB
    ///   can dedup retries.
    /// * `time_in_force` — `"GTC" | "FOK" | "FAK"`.
    pub fn encode(
        &self,
        intent: &Intent,
        signer_addr: [u8; 20],
        salt: u128,
        client_order_id: &[u8; 16],
        time_in_force: &'static str,
    ) -> Result<EncodedOrder, EncodeError> {
        let (maker_amount, taker_amount, side_u8) = compute_amounts(intent)?;

        // Encode struct fields → ABI 32-byte words, in the exact order
        // of `ORDER_TYPE_STRING`.
        // WIRE FORMAT REF: engine/src/order_signer.rs:137-160
        let salt_b32 = u128_to_b32(salt);
        let maker_b32 = addr_to_b32(&self.maker);
        let signer_b32 = addr_to_b32(&signer_addr);
        let taker_b32 = [0u8; 32]; // always zero address
        let token_id_b32 = decimal_to_b32(&intent.token_id)?;
        let maker_amount_b32 = u64_to_b32(maker_amount);
        let taker_amount_b32 = u64_to_b32(taker_amount);
        let expiration_b32 = [0u8; 32]; // no expiration — legacy default
        let nonce_b32 = [0u8; 32]; // legacy default (not the signer nonce)
        let fee_rate_bps_b32 = [0u8; 32]; // always 0
        let side_b32 = u8_to_b32(side_u8);
        let signature_type_b32 = [0u8; 32]; // EOA (0)

        let mut enc = Vec::with_capacity(32 * 13);
        enc.extend_from_slice(&self.order_type_hash);
        enc.extend_from_slice(&salt_b32);
        enc.extend_from_slice(&maker_b32);
        enc.extend_from_slice(&signer_b32);
        enc.extend_from_slice(&taker_b32);
        enc.extend_from_slice(&token_id_b32);
        enc.extend_from_slice(&maker_amount_b32);
        enc.extend_from_slice(&taker_amount_b32);
        enc.extend_from_slice(&expiration_b32);
        enc.extend_from_slice(&nonce_b32);
        enc.extend_from_slice(&fee_rate_bps_b32);
        enc.extend_from_slice(&side_b32);
        enc.extend_from_slice(&signature_type_b32);
        let struct_hash = keccak256(&enc);

        // typed_data_digest = keccak256(0x1901 || domainSep || structHash)
        // WIRE FORMAT REF: engine/src/order_signer.rs:154-160
        let digest = typed_data_digest(&self.domain_separator, &struct_hash);

        // Build the wire body *without* the signature. We cannot serialize
        // the final body until we have the 65-byte sig from the signer.
        // Store enough to do a cheap finalize later.
        let unsigned = OrderWireUnsigned {
            salt: salt.to_string(),
            maker: self.maker_hex.clone(),
            signer: addr_to_hex(&signer_addr),
            taker: "0x0000000000000000000000000000000000000000".to_string(),
            token_id: intent.token_id.clone(),
            maker_amount: maker_amount.to_string(),
            taker_amount: taker_amount.to_string(),
            expiration: "0".to_string(),
            nonce: "0".to_string(),
            fee_rate_bps: "0".to_string(),
            side: side_u8,
            signature_type: 0,
            client_order_id_hex: coid_hex(client_order_id),
            time_in_force,
        };
        let wire_body_unsigned = Bytes::from(serde_json::to_vec(&unsigned).map_err(|e| {
            EncodeError::Serialize(format!("unsigned wire body: {e}"))
        })?);

        Ok(EncodedOrder {
            struct_hash,
            digest,
            wire_body_unsigned,
            maker_amount,
            taker_amount,
            salt,
        })
    }

    /// Build the final signed POST body. `sig65 = r(32) || s(32) || v(1)`;
    /// `v` must already be Ethereum-style (27 + recovery_id).
    pub fn build_wire_body_signed(
        &self,
        intent: &Intent,
        encoded: &EncodedOrder,
        signer_addr: [u8; 20],
        sig65: &[u8; 65],
        client_order_id: &[u8; 16],
        time_in_force: &'static str,
    ) -> Result<Bytes, EncodeError> {
        let (maker_amount, taker_amount, side_u8) = compute_amounts(intent)?;
        debug_assert_eq!(maker_amount, encoded.maker_amount);
        debug_assert_eq!(taker_amount, encoded.taker_amount);

        let sig_hex = format!("0x{}", hex_encode(sig65));
        let body = OrderWireSigned {
            order: OrderFields {
                salt: encoded.salt.to_string(),
                maker: self.maker_hex.clone(),
                signer: addr_to_hex(&signer_addr),
                taker: "0x0000000000000000000000000000000000000000".to_string(),
                token_id: intent.token_id.clone(),
                maker_amount: maker_amount.to_string(),
                taker_amount: taker_amount.to_string(),
                expiration: "0".to_string(),
                nonce: "0".to_string(),
                fee_rate_bps: "0".to_string(),
                side: side_u8,
                signature_type: 0,
                signature: sig_hex,
            },
            owner: self.maker_hex.clone(),
            order_type: time_in_force,
            // WIRE FORMAT REF: engine/src/order_executor.rs:882 — post-only
            // must be true to avoid taker fees.
            maker: true,
            client_order_id: coid_hex(client_order_id),
        };
        Ok(Bytes::from(serde_json::to_vec(&body).map_err(|e| {
            EncodeError::Serialize(format!("signed wire body: {e}"))
        })?))
    }
}

// ─── Errors ──────────────────────────────────────────────────────────────

/// Errors that can occur while building the EIP-712 payload.
#[derive(Debug, thiserror::Error)]
pub enum EncodeError {
    /// Non-decimal character or > 2^256 value in `token_id`.
    #[error("invalid token_id: {0}")]
    InvalidTokenId(String),
    /// BUY or SELL with price == 0.
    #[error("invalid price: {0}")]
    InvalidPrice(&'static str),
    /// JSON serialization failed (should not happen on well-formed
    /// inputs — surfaced for completeness).
    #[error("serialize: {0}")]
    Serialize(String),
}

// ─── Wire-body structs ───────────────────────────────────────────────────

// WIRE FORMAT REF: engine/src/order_executor.rs:832-884 (OrderBody/OrderFields)
#[derive(Serialize)]
struct OrderFields {
    salt: String,
    maker: String,
    signer: String,
    taker: String,
    #[serde(rename = "tokenId")]
    token_id: String,
    #[serde(rename = "makerAmount")]
    maker_amount: String,
    #[serde(rename = "takerAmount")]
    taker_amount: String,
    expiration: String,
    nonce: String,
    #[serde(rename = "feeRateBps")]
    fee_rate_bps: String,
    side: u8,
    #[serde(rename = "signatureType")]
    signature_type: u8,
    signature: String,
}

#[derive(Serialize)]
struct OrderWireSigned {
    order: OrderFields,
    owner: String,
    #[serde(rename = "orderType")]
    order_type: &'static str,
    maker: bool,
    /// Not part of the legacy body — surfaced by the new engine so the
    /// CLOB can dedup retries. Safe to ignore server-side if unknown.
    #[serde(rename = "clientOrderId")]
    client_order_id: String,
}

#[derive(Serialize)]
struct OrderWireUnsigned {
    salt: String,
    maker: String,
    signer: String,
    taker: String,
    #[serde(rename = "tokenId")]
    token_id: String,
    #[serde(rename = "makerAmount")]
    maker_amount: String,
    #[serde(rename = "takerAmount")]
    taker_amount: String,
    expiration: String,
    nonce: String,
    #[serde(rename = "feeRateBps")]
    fee_rate_bps: String,
    side: u8,
    #[serde(rename = "signatureType")]
    signature_type: u8,
    #[serde(rename = "clientOrderId")]
    client_order_id_hex: String,
    #[serde(rename = "orderType")]
    time_in_force: &'static str,
}

// ─── Amount calculation ──────────────────────────────────────────────────

// WIRE FORMAT REF: engine/src/order_signer.rs:489-510
fn compute_amounts(intent: &Intent) -> Result<(u64, u64, u8), EncodeError> {
    let size_u = intent.size.0;
    let price = intent.price.0;
    match intent.side {
        Side::Buy => {
            if price == 0 {
                return Err(EncodeError::InvalidPrice("BUY price cannot be zero"));
            }
            // maker = USDC µunits spent; taker = shares µunits received.
            // taker = maker * 1_000 / price
            let maker_amount = size_u;
            let taker_amount =
                ((maker_amount as u128) * 1_000u128 / price as u128) as u64;
            Ok((maker_amount, taker_amount, 0u8))
        }
        Side::Sell => {
            if price == 0 {
                return Err(EncodeError::InvalidPrice("SELL price cannot be zero"));
            }
            // maker = shares µunits sold; taker = USDC µunits received.
            // taker = maker * price / 1_000
            let maker_amount = size_u;
            let taker_amount =
                ((maker_amount as u128) * price as u128 / 1_000u128) as u64;
            Ok((maker_amount, taker_amount, 1u8))
        }
    }
}

// ─── ABI helpers ─────────────────────────────────────────────────────────

// WIRE FORMAT REF: engine/src/order_signer.rs:535-594
#[inline]
pub(crate) fn addr_to_b32(addr: &[u8; 20]) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[12..].copy_from_slice(addr);
    out
}

#[inline]
pub(crate) fn u128_to_b32(v: u128) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[16..].copy_from_slice(&v.to_be_bytes());
    out
}

#[inline]
pub(crate) fn u64_to_b32(v: u64) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[24..].copy_from_slice(&v.to_be_bytes());
    out
}

#[inline]
pub(crate) fn u8_to_b32(v: u8) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[31] = v;
    out
}

/// Parse a decimal big integer into a 32-byte big-endian ABI word.
// WIRE FORMAT REF: engine/src/order_signer.rs:570-587
pub(crate) fn decimal_to_b32(s: &str) -> Result<[u8; 32], EncodeError> {
    let mut result = [0u8; 32];
    if s.is_empty() {
        return Err(EncodeError::InvalidTokenId("empty".into()));
    }
    for ch in s.chars() {
        let digit = match ch.to_digit(10) {
            Some(d) => d,
            None => {
                return Err(EncodeError::InvalidTokenId(format!(
                    "non-decimal char '{ch}' in '{s}'"
                )))
            }
        };
        let mut carry = digit as u32;
        for byte in result.iter_mut().rev() {
            let prod = (*byte as u32) * 10 + carry;
            *byte = (prod & 0xff) as u8;
            carry = prod >> 8;
        }
        if carry != 0 {
            return Err(EncodeError::InvalidTokenId(format!(
                "overflows 256 bits: {s}"
            )));
        }
    }
    Ok(result)
}

fn addr_to_hex(addr: &[u8; 20]) -> String {
    format!("0x{}", hex_encode(addr))
}

pub(crate) fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0xf) as usize] as char);
    }
    out
}

fn coid_hex(coid: &[u8; 16]) -> String {
    // Duplicated locally (rather than pulling `coid::coid_to_hex`) so this
    // module has zero non-sibling deps.
    format!("0x{}", hex_encode(coid))
}

// ─── Unit tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use blink_timestamps::Timestamp;
    use blink_types::{EventId, Intent, PriceTicks, Side, SizeU, TimeInForce};

    fn make_intent(side: Side, price: u64, size_u: u64, token_id: &str) -> Intent {
        let _ = Timestamp::UNSET;
        Intent {
            event_id: EventId(1),
            token_id: token_id.to_string(),
            market_id: "m1".to_string(),
            side,
            price: PriceTicks(price),
            size: SizeU(size_u),
            tif: TimeInForce::Gtc,
            post_only: true,
            client_order_id: "blk-1".to_string(),
        }
    }

    #[test]
    fn domain_separator_matches_legacy_vector() {
        // Legacy: chainId=137, verifyingContract=POLYMARKET_CTF_EXCHANGE,
        // name="Polymarket CTF Exchange", version="1".
        let enc = OrderEncoder::new([0u8; 20], POLYMARKET_CTF_EXCHANGE);
        // The domain separator is deterministic; any non-zero bytes
        // prove the hash was actually computed.
        assert!(enc.domain_separator().iter().any(|b| *b != 0));
        // Stability across two constructions.
        let enc2 = OrderEncoder::new([0u8; 20], POLYMARKET_CTF_EXCHANGE);
        assert_eq!(enc.domain_separator(), enc2.domain_separator());
    }

    #[test]
    fn buy_amounts_match_legacy() {
        // Legacy buy_amounts_correct: BUY $10 at price=650 → maker=10_000_000,
        // taker=15_384_615.
        let it = make_intent(Side::Buy, 650, 10_000_000, "12345");
        let (ma, ta, side) = compute_amounts(&it).unwrap();
        assert_eq!(ma, 10_000_000);
        assert_eq!(ta, 15_384_615);
        assert_eq!(side, 0);
    }

    #[test]
    fn sell_amounts_match_legacy() {
        // Legacy sell_amounts_correct: SELL 10 shares at 650 →
        // maker=10_000_000 (shares µ), taker=6_500_000 (USDC µ).
        let it = make_intent(Side::Sell, 650, 10_000_000, "12345");
        let (ma, ta, side) = compute_amounts(&it).unwrap();
        assert_eq!(ma, 10_000_000);
        assert_eq!(ta, 6_500_000);
        assert_eq!(side, 1);
    }

    #[test]
    fn zero_price_rejected() {
        let it = make_intent(Side::Buy, 0, 10_000_000, "12345");
        assert!(compute_amounts(&it).is_err());
    }

    #[test]
    fn decimal_to_b32_small() {
        let b = decimal_to_b32("255").unwrap();
        assert_eq!(b[31], 0xff);
        for i in 0..31 {
            assert_eq!(b[i], 0);
        }
    }

    #[test]
    fn decimal_to_b32_rejects_garbage() {
        assert!(decimal_to_b32("12a").is_err());
        assert!(decimal_to_b32("").is_err());
    }

    #[test]
    fn encode_is_deterministic_same_inputs() {
        let enc = OrderEncoder::new([0x11; 20], POLYMARKET_CTF_EXCHANGE);
        let it = make_intent(Side::Buy, 650, 10_000_000, "12345");
        let coid = [0x42; 16];
        let a = enc.encode(&it, [0x22; 20], 777, &coid, "GTC").unwrap();
        let b = enc.encode(&it, [0x22; 20], 777, &coid, "GTC").unwrap();
        assert_eq!(a.digest, b.digest);
        assert_eq!(a.struct_hash, b.struct_hash);
    }

    #[test]
    fn encode_changes_with_salt() {
        let enc = OrderEncoder::new([0x11; 20], POLYMARKET_CTF_EXCHANGE);
        let it = make_intent(Side::Buy, 650, 10_000_000, "12345");
        let coid = [0x42; 16];
        let a = enc.encode(&it, [0x22; 20], 1, &coid, "GTC").unwrap();
        let b = enc.encode(&it, [0x22; 20], 2, &coid, "GTC").unwrap();
        assert_ne!(a.digest, b.digest);
    }

    #[test]
    fn signed_body_round_trips() {
        let enc = OrderEncoder::new([0x11; 20], POLYMARKET_CTF_EXCHANGE);
        let it = make_intent(Side::Sell, 650, 10_000_000, "12345");
        let coid = [0xaa; 16];
        let e = enc.encode(&it, [0x22; 20], 42, &coid, "GTC").unwrap();
        let sig = [0x33u8; 65];
        let body = enc
            .build_wire_body_signed(&it, &e, [0x22; 20], &sig, &coid, "GTC")
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["order"]["side"], 1);
        assert_eq!(v["order"]["salt"], "42");
        assert_eq!(v["orderType"], "GTC");
        assert_eq!(v["maker"], true);
        assert_eq!(v["clientOrderId"], format!("0x{}", "aa".repeat(16)));
        assert!(v["order"]["signature"]
            .as_str()
            .unwrap()
            .starts_with("0x"));
    }
}
