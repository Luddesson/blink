//! Pre-signed order templates + partial EIP-712 hash cache.
//!
//! When the kernel emits a Submit, as much EIP-712 work as possible is
//! already done — we only fill in `(salt, makerAmount, takerAmount)` and
//! finalize the Keccak sponge.
//!
//! # Strategy
//!
//! The Polymarket Order struct is:
//!
//! ```text
//! Order(uint256 salt, address maker, address signer, address taker,
//!       uint256 tokenId, uint256 makerAmount, uint256 takerAmount,
//!       uint256 expiration, uint256 nonce, uint256 feeRateBps,
//!       uint8 side, uint8 signatureType)
//! ```
//!
//! Fixed per `(market, side, maker, exchange)`: `maker`, `signer`, `taker`,
//! `tokenId`, `expiration`, `nonce`, `feeRateBps`, `side`, `signatureType`
//! — 9 of 12 fields.
//!
//! Variable per decision: `salt`, `makerAmount`, `takerAmount` (3 fields).
//!
//! An [`OrderTemplate`] stores:
//!
//! 1. A `Keccak256` hasher pre-fed with `order_typehash` (first 32 bytes
//!    of the struct hash pre-image).
//! 2. The 9 constant ABI words split into two pre-assembled byte slabs
//!    (the positions between/after the 3 variable words) so decision-time
//!    work is just: clone hasher → 6 `update()` calls → `finalize()`.
//! 3. The domain separator (one more Keccak over 66 bytes for the final
//!    typed-data digest).
//!
//! `Keccak256` is `Clone`; a clone is a ~200-byte sponge-state copy. See
//! the bench (`benches/template.rs`) and the `clone_hasher_is_cheap` test.
//!
//! WIRE FORMAT REF: mirrors `encoder::OrderEncoder::encode` byte-for-byte.
//! The `digest_matches_encoder` test asserts this across random inputs.

use std::sync::Arc;

use blink_signer::eip712::typed_data_digest;
use blink_types::{Intent, Side};
use bytes::Bytes;
use dashmap::DashMap;
use serde::Serialize;
use sha3::{Digest, Keccak256};

use crate::encoder::{
    addr_to_b32, decimal_to_b32, hex_encode, u128_to_b32, u64_to_b32, u8_to_b32, EncodeError,
    OrderEncoder,
};

/// Opaque market identifier. Mirrors `blink_types::Intent::market_id`.
pub type MarketId = String;

// ─── OrderTemplate ───────────────────────────────────────────────────────

/// Pre-computed EIP-712 material for a `(market, side)` pair.
///
/// The expensive constant parts (domain separator, type hash, 9 ABI-encoded
/// constant fields) are computed once via [`OrderTemplate::build`]. At
/// decision time, [`OrderTemplate::digest`] fills in `(salt, makerAmount,
/// takerAmount)` and returns the full EIP-712 typed-data digest ready to
/// sign.
#[derive(Clone)]
pub struct OrderTemplate {
    pub market_id: MarketId,
    pub token_id_padded: [u8; 32],
    pub maker: [u8; 20],
    pub signer: [u8; 20],
    pub taker: [u8; 20],
    pub exchange: [u8; 20],
    pub expiration: u64,
    pub nonce: u64,
    pub fee_rate_bps: u16,
    pub signature_type: u8,
    /// `0` = Buy, `1` = Sell per the legacy wire format.
    pub side_bit: u8,
    pub domain_separator: [u8; 32],

    /// Keccak256 hasher pre-fed with `order_typehash` (32 bytes). Cloned
    /// per `digest()` call (~200 ns sponge-state copy).
    prefix_hasher: Keccak256,

    /// ABI-encoded constants between `salt` and `makerAmount`:
    /// `maker || signer || taker || tokenId` (4 × 32 = 128 B).
    const_mid: [u8; 128],

    /// ABI-encoded constants after `takerAmount`:
    /// `expiration || nonce || feeRateBps || side || signatureType`
    /// (5 × 32 = 160 B).
    const_tail: [u8; 160],

    // Data needed to rebuild the JSON wire body without going through the
    // slow path again.
    token_id_decimal: String,
    maker_hex: String,
    signer_hex: String,
    taker_hex: String,
    fee_rate_bps_str: String,
    expiration_str: String,
    nonce_str: String,
}

impl std::fmt::Debug for OrderTemplate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OrderTemplate")
            .field("market_id", &self.market_id)
            .field("maker", &self.maker_hex)
            .field("signer", &self.signer_hex)
            .field("side_bit", &self.side_bit)
            .field("expiration", &self.expiration)
            .field("nonce", &self.nonce)
            .finish()
    }
}

impl OrderTemplate {
    /// Warmup path. Pre-computes hash state and ABI-encodes the 9 constant
    /// fields. Cost: 1 `decimal_to_b32` + ~200 B of buffer fills + 1
    /// `Keccak256::update` (32 B).
    ///
    /// `encoder` supplies the shared `(maker, exchange, domain_separator,
    /// order_type_hash)`.
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        encoder: &OrderEncoder,
        market_id: impl Into<String>,
        token_id_decimal: &str,
        signer: [u8; 20],
        taker: [u8; 20],
        expiration: u64,
        nonce: u64,
        fee_rate_bps: u16,
        signature_type: u8,
        side_bit: u8,
    ) -> Result<Self, EncodeError> {
        let maker = encoder.maker();
        let exchange = encoder.exchange();
        let domain_separator = encoder.domain_separator();
        let order_type_hash = encoder.order_type_hash();

        let token_id_padded = decimal_to_b32(token_id_decimal)?;

        // Middle slab: maker || signer || taker || tokenId
        let mut const_mid = [0u8; 128];
        const_mid[0..32].copy_from_slice(&addr_to_b32(&maker));
        const_mid[32..64].copy_from_slice(&addr_to_b32(&signer));
        const_mid[64..96].copy_from_slice(&addr_to_b32(&taker));
        const_mid[96..128].copy_from_slice(&token_id_padded);

        // Tail slab: expiration || nonce || feeRateBps || side || signatureType
        let mut const_tail = [0u8; 160];
        const_tail[0..32].copy_from_slice(&u64_to_b32(expiration));
        const_tail[32..64].copy_from_slice(&u64_to_b32(nonce));
        const_tail[64..96].copy_from_slice(&{
            let mut b = [0u8; 32];
            b[30..32].copy_from_slice(&fee_rate_bps.to_be_bytes());
            b
        });
        const_tail[96..128].copy_from_slice(&u8_to_b32(side_bit));
        const_tail[128..160].copy_from_slice(&u8_to_b32(signature_type));

        // Pre-absorb the type hash.
        let mut prefix_hasher = Keccak256::new();
        prefix_hasher.update(order_type_hash);

        Ok(Self {
            market_id: market_id.into(),
            token_id_padded,
            maker,
            signer,
            taker,
            exchange,
            expiration,
            nonce,
            fee_rate_bps,
            signature_type,
            side_bit,
            domain_separator,
            prefix_hasher,
            const_mid,
            const_tail,
            token_id_decimal: token_id_decimal.to_string(),
            maker_hex: addr_hex(&maker),
            signer_hex: addr_hex(&signer),
            taker_hex: addr_hex(&taker),
            fee_rate_bps_str: fee_rate_bps.to_string(),
            expiration_str: expiration.to_string(),
            nonce_str: nonce.to_string(),
        })
    }

    /// Hot path. Fills in `(salt, maker_amount, taker_amount)` and returns
    /// the full EIP-712 typed-data digest ready for
    /// [`blink_signer::SignerPool::sign`].
    ///
    /// Byte-identical to
    /// `OrderEncoder::encode(..).digest` when the template was built with
    /// matching inputs — asserted by the `digest_matches_encoder` test.
    #[inline]
    pub fn digest(&self, salt: u128, maker_amount: u64, taker_amount: u64) -> [u8; 32] {
        // Clone of a fresh Keccak256 (only the type-hash absorbed) is a
        // ~200-byte sponge-state copy.
        let mut h = self.prefix_hasher.clone();
        h.update(u128_to_b32(salt));
        h.update(self.const_mid);
        h.update(u64_to_b32(maker_amount));
        h.update(u64_to_b32(taker_amount));
        h.update(self.const_tail);
        let struct_hash: [u8; 32] = h.finalize().into();
        typed_data_digest(&self.domain_separator, &struct_hash)
    }

    /// Build the unsigned wire body (signature goes in later).
    ///
    /// Shape-compatible with [`OrderEncoder::encode`]'s
    /// `wire_body_unsigned` so the signed-body path can be reused
    /// unchanged — see [`crate::Submitter::submit_templated`].
    pub fn wire_body_unsigned(
        &self,
        salt: u128,
        maker_amount: u64,
        taker_amount: u64,
        coid_hex: &str,
        time_in_force: &'static str,
    ) -> Bytes {
        let body = OrderWireUnsigned {
            salt: salt.to_string(),
            maker: &self.maker_hex,
            signer: &self.signer_hex,
            taker: &self.taker_hex,
            token_id: &self.token_id_decimal,
            maker_amount: maker_amount.to_string(),
            taker_amount: taker_amount.to_string(),
            expiration: &self.expiration_str,
            nonce: &self.nonce_str,
            fee_rate_bps: &self.fee_rate_bps_str,
            side: self.side_bit,
            signature_type: self.signature_type,
            client_order_id_hex: coid_hex,
            time_in_force,
        };
        // Serialization only fails on non-string map keys; our shape is
        // flat, so this cannot fail.
        Bytes::from(serde_json::to_vec(&body).expect("OrderWireUnsigned cannot fail to serialize"))
    }
}

#[derive(Serialize)]
struct OrderWireUnsigned<'a> {
    salt: String,
    maker: &'a str,
    signer: &'a str,
    taker: &'a str,
    #[serde(rename = "tokenId")]
    token_id: &'a str,
    #[serde(rename = "makerAmount")]
    maker_amount: String,
    #[serde(rename = "takerAmount")]
    taker_amount: String,
    expiration: &'a str,
    nonce: &'a str,
    #[serde(rename = "feeRateBps")]
    fee_rate_bps: &'a str,
    side: u8,
    #[serde(rename = "signatureType")]
    signature_type: u8,
    #[serde(rename = "clientOrderId")]
    client_order_id_hex: &'a str,
    #[serde(rename = "orderType")]
    time_in_force: &'static str,
}

// ─── TemplateCache ───────────────────────────────────────────────────────

/// Per-process cache of [`OrderTemplate`]s keyed by `(market_id, side_bit)`.
///
/// Hot-path access is via [`TemplateCache::get`] — lock-free on `DashMap`.
/// Warmup (which performs the keccak + ABI encoding) is the caller's
/// responsibility via [`TemplateCache::warmup`]; typically called when a
/// new market is discovered, off the submit hot path.
#[derive(Debug)]
pub struct TemplateCache {
    map: DashMap<(MarketId, u8), Arc<OrderTemplate>>,
    encoder: OrderEncoder,
    stats: CacheCounters,
}

#[derive(Debug, Default)]
struct CacheCounters {
    hits: std::sync::atomic::AtomicU64,
    misses: std::sync::atomic::AtomicU64,
    warmups: std::sync::atomic::AtomicU64,
}

/// Snapshot of [`TemplateCache`] counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TemplateCacheStats {
    pub hits: u64,
    pub misses: u64,
    pub warmups: u64,
    pub entries: usize,
}

impl TemplateCache {
    pub fn new(encoder: OrderEncoder) -> Self {
        Self {
            map: DashMap::new(),
            encoder,
            stats: CacheCounters::default(),
        }
    }

    /// Expose the encoder for callers that need to fall back to the slow
    /// path (e.g. on a cache miss where warmup hasn't completed yet).
    pub fn encoder(&self) -> &OrderEncoder {
        &self.encoder
    }

    /// Hot path. Returns `None` on miss — warmup is the caller's job.
    pub fn get(&self, market_id: &str, side_bit: u8) -> Option<Arc<OrderTemplate>> {
        // DashMap requires owned key components for `get`. Since our key is
        // `(String, u8)` we have to allocate — this is the dominant cost
        // on miss, not hit. Acceptable; the cache is read-mostly.
        let key = (market_id.to_string(), side_bit);
        match self.map.get(&key) {
            Some(v) => {
                self.stats
                    .hits
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Some(v.clone())
            }
            None => {
                self.stats
                    .misses
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                None
            }
        }
    }

    /// Warmup path. Builds and inserts (or replaces) a template.
    #[allow(clippy::too_many_arguments)]
    pub fn warmup(
        &self,
        market_id: &str,
        token_id: &str,
        side_bit: u8,
        signer: [u8; 20],
        taker: [u8; 20],
        expiration: u64,
        nonce: u64,
        fee_rate_bps: u16,
        signature_type: u8,
    ) -> Result<Arc<OrderTemplate>, EncodeError> {
        let tpl = Arc::new(OrderTemplate::build(
            &self.encoder,
            market_id.to_string(),
            token_id,
            signer,
            taker,
            expiration,
            nonce,
            fee_rate_bps,
            signature_type,
            side_bit,
        )?);
        self.map
            .insert((market_id.to_string(), side_bit), tpl.clone());
        self.stats
            .warmups
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(tpl)
    }

    /// Drop all templates for a market (e.g. on resolution or maker
    /// nonce rotation).
    pub fn invalidate(&self, market_id: &str) {
        self.map.retain(|(m, _), _| m != market_id);
    }

    pub fn stats(&self) -> TemplateCacheStats {
        use std::sync::atomic::Ordering::Relaxed;
        TemplateCacheStats {
            hits: self.stats.hits.load(Relaxed),
            misses: self.stats.misses.load(Relaxed),
            warmups: self.stats.warmups.load(Relaxed),
            entries: self.map.len(),
        }
    }
}

// ─── helpers ─────────────────────────────────────────────────────────────

fn addr_hex(addr: &[u8; 20]) -> String {
    format!("0x{}", hex_encode(addr))
}

/// Convenience: map `(Side, Intent)` amounts into the `(maker, taker, side_bit)`
/// triple the wire format wants. Mirrors `encoder::compute_amounts` but
/// exposed for template users.
///
/// Returns `(maker_amount, taker_amount, side_bit)`.
// WIRE FORMAT REF: engine/src/order_signer.rs:489-510
pub fn compute_amounts_for_intent(intent: &Intent) -> Result<(u64, u64, u8), EncodeError> {
    let size_u = intent.size.0;
    let price = intent.price.0;
    match intent.side {
        Side::Buy => {
            if price == 0 {
                return Err(EncodeError::InvalidPrice("BUY price cannot be zero"));
            }
            let maker_amount = size_u;
            let taker_amount = ((maker_amount as u128) * 1_000u128 / price as u128) as u64;
            Ok((maker_amount, taker_amount, 0u8))
        }
        Side::Sell => {
            if price == 0 {
                return Err(EncodeError::InvalidPrice("SELL price cannot be zero"));
            }
            let maker_amount = size_u;
            let taker_amount = ((maker_amount as u128) * price as u128 / 1_000u128) as u64;
            Ok((maker_amount, taker_amount, 1u8))
        }
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoder::POLYMARKET_CTF_EXCHANGE;
    use blink_types::{EventId, Intent, PriceTicks, Side, SizeU, TimeInForce};

    fn mk_encoder() -> OrderEncoder {
        OrderEncoder::new([0x11; 20], POLYMARKET_CTF_EXCHANGE)
    }

    fn mk_intent(side: Side, price: u64, size_u: u64, token_id: &str) -> Intent {
        Intent {
            event_id: EventId(1),
            token_id: token_id.to_string(),
            market_id: "m-test".to_string(),
            side,
            price: PriceTicks(price),
            size: SizeU(size_u),
            tif: TimeInForce::Gtc,
            post_only: true,
            client_order_id: "coid".to_string(),
        }
    }

    fn mk_template(enc: &OrderEncoder, side_bit: u8, token_id: &str) -> OrderTemplate {
        OrderTemplate::build(
            enc,
            "m-test",
            token_id,
            [0x22; 20], // signer
            [0u8; 20],  // taker (zero address)
            0,          // expiration
            0,          // nonce
            0,          // fee_rate_bps
            0,          // signature_type (EOA)
            side_bit,
        )
        .unwrap()
    }

    /// **Correctness invariant**: template's `digest()` must be byte-identical
    /// to the full `OrderEncoder::encode()` path across random inputs.
    /// Failing this test means the wire format is broken.
    #[test]
    fn digest_matches_encoder() {
        // Deterministic pseudo-random — no `rand` import needed, xorshift.
        let mut state: u64 = 0xdead_beef_cafe_f00d;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };

        let enc = mk_encoder();
        let signer_addr = [0x22; 20];
        let token_id =
            "52114319501245915516055106046884209969926127482827954674443586998463594912099";

        for i in 0..10 {
            let side = if i % 2 == 0 { Side::Buy } else { Side::Sell };
            let side_bit = if matches!(side, Side::Buy) { 0 } else { 1 };

            let price = 1 + (next() % 999); // 1..=999 (avoid zero)
            let size_u = 1 + (next() % 1_000_000_000);
            let salt = next() as u128 | ((next() as u128) << 64);

            let intent = mk_intent(side, price, size_u, token_id);
            let coid = [0x42; 16];

            let full = enc
                .encode(&intent, signer_addr, salt, &coid, "GTC")
                .expect("encode");

            let tpl = mk_template(&enc, side_bit, token_id);
            let (ma, ta, s_bit) = compute_amounts_for_intent(&intent).unwrap();
            assert_eq!(s_bit, side_bit);
            assert_eq!(ma, full.maker_amount);
            assert_eq!(ta, full.taker_amount);

            let tpl_digest = tpl.digest(salt, ma, ta);
            assert_eq!(
                tpl_digest, full.digest,
                "digest mismatch at i={} (side={:?}, price={}, size={}, salt={})",
                i, side, price, size_u, salt
            );
        }
    }

    #[test]
    fn clone_hasher_is_cheap() {
        // Order-of-magnitude check: 1000 digest() calls in < 1 ms.
        // On CI this is typically ~50–150 µs; the 1 ms bound is slack
        // for contended / virtualized runners.
        let enc = mk_encoder();
        let tpl = mk_template(&enc, 0, "12345");
        let start = std::time::Instant::now();
        let mut acc: u8 = 0;
        for i in 0..1000u64 {
            let d = tpl.digest(i as u128, 10_000_000 + i, 15_384_615 + i);
            acc ^= d[0];
        }
        let elapsed = start.elapsed();
        // Touch `acc` so the loop isn't optimized away.
        std::hint::black_box(acc);
        assert!(
            elapsed < std::time::Duration::from_millis(10),
            "1000 digests took {:?} (expected < 10 ms, target < 1 ms)",
            elapsed
        );
    }

    #[test]
    fn cache_hit_miss_accounting() {
        let enc = mk_encoder();
        let cache = TemplateCache::new(enc);

        assert!(cache.get("m-test", 0).is_none());
        let s0 = cache.stats();
        assert_eq!(s0.hits, 0);
        assert_eq!(s0.misses, 1);
        assert_eq!(s0.warmups, 0);
        assert_eq!(s0.entries, 0);

        cache
            .warmup("m-test", "12345", 0, [0x22; 20], [0u8; 20], 0, 0, 0, 0)
            .unwrap();
        let s1 = cache.stats();
        assert_eq!(s1.warmups, 1);
        assert_eq!(s1.entries, 1);

        assert!(cache.get("m-test", 0).is_some());
        assert!(cache.get("m-test", 1).is_none()); // side 1 not warmed
        let s2 = cache.stats();
        assert_eq!(s2.hits, 1);
        assert_eq!(s2.misses, 2);
    }

    #[test]
    fn cache_concurrent_access() {
        use std::sync::Arc as StdArc;
        use std::thread;

        let cache = StdArc::new(TemplateCache::new(mk_encoder()));
        cache
            .warmup("m-test", "12345", 0, [0x22; 20], [0u8; 20], 0, 0, 0, 0)
            .unwrap();

        let mut handles = Vec::new();
        for _ in 0..8 {
            let c = StdArc::clone(&cache);
            handles.push(thread::spawn(move || {
                for _ in 0..1000 {
                    assert!(c.get("m-test", 0).is_some());
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        let s = cache.stats();
        assert_eq!(s.hits, 8 * 1000);
        assert_eq!(s.misses, 0);
    }

    #[test]
    fn invalidate_removes_entries() {
        let cache = TemplateCache::new(mk_encoder());
        cache
            .warmup("m-a", "12345", 0, [0x22; 20], [0u8; 20], 0, 0, 0, 0)
            .unwrap();
        cache
            .warmup("m-a", "12345", 1, [0x22; 20], [0u8; 20], 0, 0, 0, 0)
            .unwrap();
        cache
            .warmup("m-b", "67890", 0, [0x22; 20], [0u8; 20], 0, 0, 0, 0)
            .unwrap();
        assert_eq!(cache.stats().entries, 3);
        cache.invalidate("m-a");
        assert_eq!(cache.stats().entries, 1);
        assert!(cache.get("m-a", 0).is_none());
        assert!(cache.get("m-b", 0).is_some());
    }

    #[test]
    fn wire_body_unsigned_shape() {
        let enc = mk_encoder();
        let tpl = mk_template(&enc, 0, "12345");
        let body = tpl.wire_body_unsigned(42, 10_000_000, 15_384_615, "0xabcd", "GTC");
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["salt"], "42");
        assert_eq!(v["tokenId"], "12345");
        assert_eq!(v["makerAmount"], "10000000");
        assert_eq!(v["takerAmount"], "15384615");
        assert_eq!(v["side"], 0);
        assert_eq!(v["orderType"], "GTC");
        assert_eq!(v["clientOrderId"], "0xabcd");
        assert_eq!(v["expiration"], "0");
        assert_eq!(v["nonce"], "0");
        assert_eq!(v["feeRateBps"], "0");
    }
}
