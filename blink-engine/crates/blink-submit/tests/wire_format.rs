//! Wire-format parity test for `OrderEncoder`.
//!
//! The "expected" digest is computed two ways in this test:
//!
//! 1. By [`blink_submit::OrderEncoder::encode`] — the production path.
//!
//! 2. By a faithful in-test reimplementation of
//!    `engine/src/order_signer.rs::sign_order_deterministic` using raw
//!    `sha3::Keccak256` primitives. This acts as the pinned "legacy
//!    reference" vector: if the production encoder drifts from the
//!    legacy wire format, this test fails with a clear byte-diff.
//!
//! The test also asserts against a hard-coded hex dump of the digest
//! captured from the reference implementation; that guards against
//! both the production path *and* the reference path silently changing
//! in lock-step.
//!
//! # Where the hard-coded digest came from
//!
//! The first run of this test (dev: 2024-Q4) printed the digest; that
//! value is pasted back into `EXPECTED_DIGEST_HEX` below. Any future
//! divergence in either algorithm will flip this test red and require a
//! conscious update + re-review of `engine/src/order_signer.rs`.
//!
//! WIRE FORMAT REF: engine/src/order_signer.rs:208-287
//! (`sign_order_deterministic`), lines 137-160 for the Order struct
//! hash layout, lines 456-471 for the domain separator.

use blink_submit::{OrderEncoder};
use blink_types::{EventId, Intent, PriceTicks, Side, SizeU, TimeInForce};
use sha3::{Digest, Keccak256};

// ─── Test vector ─────────────────────────────────────────────────────────

const MAKER: [u8; 20] = [
    0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88,
    0x99, 0x01, 0x02, 0x03, 0x04,
];

const EXCHANGE: [u8; 20] = [
    0x4b, 0xfb, 0x41, 0xd5, 0xb3, 0x57, 0x0d, 0xef, 0xd0, 0x3c, 0x39, 0xa9, 0xa4, 0xd8, 0xde,
    0x6b, 0xd8, 0xb8, 0x98, 0x2e,
];

const SIGNER: [u8; 20] = [
    0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff,
    0x00, 0x10, 0x20, 0x30, 0x40,
];

const SALT: u128 = 0xdead_beef_cafe_f00d_1234_5678_9abc_def0u128;

fn test_intent() -> Intent {
    Intent {
        event_id: EventId(42),
        token_id: "71321045679252212594626385532706912750332728571942532289631379312455583992563"
            .to_string(),
        market_id: "0xmarket".to_string(),
        side: Side::Buy,
        price: PriceTicks(650),    // $0.65
        size: SizeU(10_000_000),    // $10.00 in USDC µunits
        tif: TimeInForce::Gtc,
        post_only: true,
        client_order_id: "0x11111111111111111111111111111111".into(),
    }
}

// ─── Reference algorithm (legacy) ────────────────────────────────────────

const ORDER_TYPE_STRING: &[u8] =
    b"Order(uint256 salt,address maker,address signer,address taker,uint256 tokenId,uint256 makerAmount,uint256 takerAmount,uint256 expiration,uint256 nonce,uint256 feeRateBps,uint8 side,uint8 signatureType)";

const DOMAIN_TYPE_STRING: &[u8] =
    b"EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)";

fn keccak(data: &[u8]) -> [u8; 32] {
    let mut h = Keccak256::new();
    h.update(data);
    h.finalize().into()
}

fn b32_addr(a: &[u8; 20]) -> [u8; 32] {
    let mut o = [0u8; 32];
    o[12..].copy_from_slice(a);
    o
}
fn b32_u128(v: u128) -> [u8; 32] {
    let mut o = [0u8; 32];
    o[16..].copy_from_slice(&v.to_be_bytes());
    o
}
fn b32_u64(v: u64) -> [u8; 32] {
    let mut o = [0u8; 32];
    o[24..].copy_from_slice(&v.to_be_bytes());
    o
}
fn b32_u8(v: u8) -> [u8; 32] {
    let mut o = [0u8; 32];
    o[31] = v;
    o
}

/// Parse a decimal big-int into a 32-byte big-endian ABI word. Mirrors
/// `engine/src/order_signer.rs::decimal_to_b32`.
fn b32_decimal(s: &str) -> [u8; 32] {
    let mut result = [0u8; 32];
    for ch in s.chars() {
        let digit = ch.to_digit(10).expect("decimal");
        let mut carry = digit as u32;
        for byte in result.iter_mut().rev() {
            let prod = (*byte as u32) * 10 + carry;
            *byte = (prod & 0xff) as u8;
            carry = prod >> 8;
        }
        assert_eq!(carry, 0, "overflow");
    }
    result
}

fn domain_sep() -> [u8; 32] {
    let type_hash = keccak(DOMAIN_TYPE_STRING);
    let name_hash = keccak(b"Polymarket CTF Exchange");
    let version_hash = keccak(b"1");
    let chain = b32_u128(137u128);
    let contract = b32_addr(&EXCHANGE);
    let mut enc = Vec::with_capacity(32 * 5);
    enc.extend_from_slice(&type_hash);
    enc.extend_from_slice(&name_hash);
    enc.extend_from_slice(&version_hash);
    enc.extend_from_slice(&chain);
    enc.extend_from_slice(&contract);
    keccak(&enc)
}

/// Compute the (struct_hash, digest) the way legacy does. Buy side,
/// expiration/nonce/feeRateBps/signatureType all zero.
fn legacy_digest(intent: &Intent, salt: u128) -> ([u8; 32], [u8; 32]) {
    let size_u = intent.size.0;
    let price = intent.price.0;
    let (maker_amount, taker_amount, side_u8) = match intent.side {
        Side::Buy => {
            let ma = size_u;
            let ta = (ma as u128 * 1_000u128 / price as u128) as u64;
            (ma, ta, 0u8)
        }
        Side::Sell => {
            let ma = size_u;
            let ta = (ma as u128 * price as u128 / 1_000u128) as u64;
            (ma, ta, 1u8)
        }
    };

    let type_hash = keccak(ORDER_TYPE_STRING);
    let mut enc = Vec::with_capacity(32 * 13);
    enc.extend_from_slice(&type_hash);
    enc.extend_from_slice(&b32_u128(salt));
    enc.extend_from_slice(&b32_addr(&MAKER));
    enc.extend_from_slice(&b32_addr(&SIGNER));
    enc.extend_from_slice(&[0u8; 32]); // taker = 0
    enc.extend_from_slice(&b32_decimal(&intent.token_id));
    enc.extend_from_slice(&b32_u64(maker_amount));
    enc.extend_from_slice(&b32_u64(taker_amount));
    enc.extend_from_slice(&[0u8; 32]); // expiration
    enc.extend_from_slice(&[0u8; 32]); // nonce
    enc.extend_from_slice(&[0u8; 32]); // feeRateBps
    enc.extend_from_slice(&b32_u8(side_u8));
    enc.extend_from_slice(&[0u8; 32]); // signatureType
    let struct_hash = keccak(&enc);

    let ds = domain_sep();
    let mut msg = Vec::with_capacity(66);
    msg.extend_from_slice(&[0x19, 0x01]);
    msg.extend_from_slice(&ds);
    msg.extend_from_slice(&struct_hash);
    (struct_hash, keccak(&msg))
}

// ─── Pinned expected digest ──────────────────────────────────────────────
// See module docs. Regenerate by first running the test and pasting the
// printed value if the wire format INTENTIONALLY changes.

const EXPECTED_DIGEST_HEX: &str =
    include_str!("fixtures/legacy_digest.hex");

// ─── Tests ───────────────────────────────────────────────────────────────

#[test]
fn encoder_matches_legacy_algorithm_byte_for_byte() {
    let intent = test_intent();
    let coid = [0x11u8; 16];
    let enc = OrderEncoder::new(MAKER, EXCHANGE);
    let out = enc
        .encode(&intent, SIGNER, SALT, &coid, "GTC")
        .expect("encode");

    let (ref_struct_hash, ref_digest) = legacy_digest(&intent, SALT);

    assert_eq!(
        out.struct_hash, ref_struct_hash,
        "struct_hash mismatch\nproduction: {}\nreference:  {}",
        hex::encode(out.struct_hash),
        hex::encode(ref_struct_hash)
    );
    assert_eq!(
        out.digest, ref_digest,
        "digest mismatch\nproduction: {}\nreference:  {}",
        hex::encode(out.digest),
        hex::encode(ref_digest)
    );

    // Pinned reference — hard-coded fixture.
    let expected = hex::decode(EXPECTED_DIGEST_HEX.trim()).expect("decode fixture");
    assert_eq!(
        &out.digest[..],
        &expected[..],
        "digest drifted from pinned fixture tests/fixtures/legacy_digest.hex\n\
         got:      {}\n\
         expected: {}\n\
         If the wire format was INTENTIONALLY changed, regenerate the \
         fixture and re-review engine/src/order_signer.rs.",
        hex::encode(out.digest),
        hex::encode(&expected),
    );
}

#[test]
fn domain_separator_matches_legacy() {
    let enc = OrderEncoder::new(MAKER, EXCHANGE);
    assert_eq!(enc.domain_separator(), domain_sep());
}

#[test]
fn signed_body_shape_matches_legacy_json_keys() {
    let intent = test_intent();
    let coid = [0x22u8; 16];
    let enc = OrderEncoder::new(MAKER, EXCHANGE);
    let out = enc.encode(&intent, SIGNER, SALT, &coid, "GTC").unwrap();
    let fake_sig = [0u8; 65];
    let body = enc
        .build_wire_body_signed(&intent, &out, SIGNER, &fake_sig, &coid, "GTC")
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();

    // Every field present on the legacy `OrderBody` must still be there,
    // with the same JSON key casing.
    // WIRE FORMAT REF: engine/src/order_executor.rs:832-884
    for k in [
        "salt",
        "maker",
        "signer",
        "taker",
        "tokenId",
        "makerAmount",
        "takerAmount",
        "expiration",
        "nonce",
        "feeRateBps",
        "side",
        "signatureType",
        "signature",
    ] {
        assert!(
            v["order"].get(k).is_some(),
            "missing JSON field order.{k}"
        );
    }
    assert_eq!(v["owner"], format!("0x{}", hex::encode(MAKER)));
    assert_eq!(v["orderType"], "GTC");
    assert_eq!(v["maker"], true);
    assert_eq!(v["clientOrderId"], format!("0x{}", hex::encode(coid)));
}
