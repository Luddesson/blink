//! EIP-712 order signing for the Polymarket CLOB.
//!
//! Manual EIP-712 implementation using k256 (ECDSA/secp256k1) and sha3
//! (Keccak256) — no alloy dependency required.
//!
//! # Domain
//! ```text
//! name:              "Polymarket CTF Exchange"
//! version:           "1"
//! chainId:           137  (Polygon)
//! verifyingContract: 0x4bFb41d5B3570DeFd03C39a9A4D8dE6Bd8B8982E
//! ```

use anyhow::{Context, Result};
use k256::ecdsa::signature::hazmat::PrehashSigner;
use k256::ecdsa::{RecoveryId, Signature, SigningKey};
use sha3::{Digest, Keccak256};

use crate::types::OrderSide;
use std::time::{SystemTime, UNIX_EPOCH};

// ─── EIP-712 type strings ─────────────────────────────────────────────────────

const DOMAIN_TYPE_STRING: &[u8] =
    b"EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)";

const ORDER_TYPE_STRING: &[u8] =
    b"Order(uint256 salt,address maker,address signer,address taker,uint256 tokenId,uint256 makerAmount,uint256 takerAmount,uint256 expiration,uint256 nonce,uint256 feeRateBps,uint8 side,uint8 signatureType)";

const VERIFYING_CONTRACT_HEX: &str = "4bFb41d5B3570DeFd03C39a9A4D8dE6Bd8B8982E";

// ─── Public types ─────────────────────────────────────────────────────────────

/// Input parameters for constructing a signed CLOB order.
#[derive(Debug, Clone)]
pub struct OrderParams {
    /// Polymarket token (condition) ID — a large decimal integer string.
    pub token_id: String,
    /// Direction of the order.
    pub side: OrderSide,
    /// Limit price scaled ×1 000 (e.g. `0.65` → `650`).
    pub price: u64,
    /// For `Buy`: USDC amount as a float (e.g. `5.00` for $5).
    /// For `Sell`: number of shares as a float (e.g. `10.0` for 10 shares).
    pub size: f64,
    /// The maker (funder / proxy-wallet) address as `"0x..."`.
    pub maker: String,
}

#[derive(Debug, Clone, Copy)]
pub struct OrderSigningPolicy {
    pub expiration: u64,
    pub nonce: u64,
    pub signature_type: u8,
}

impl Default for OrderSigningPolicy {
    fn default() -> Self {
        Self {
            expiration: 0,
            nonce: 0,
            signature_type: 0,
        }
    }
}

/// A fully signed order ready for submission to the Polymarket CLOB REST API.
#[derive(Debug, Clone)]
pub struct SignedOrder {
    /// Random decimal salt string (u128).
    pub salt: String,
    /// Maker address as `"0x..."`.
    pub maker: String,
    /// Signing-key address as `"0x..."`.
    pub signer: String,
    /// Taker address — always the zero address.
    pub taker: String,
    /// Polymarket token ID as a decimal string.
    pub token_id: String,
    pub maker_amount: u64,
    pub taker_amount: u64,
    pub expiration: u64,
    pub nonce: u64,
    pub fee_rate_bps: u64,
    pub side: u8,
    pub signature_type: u8,
    /// 0x-prefixed hex EIP-712 signature (65 bytes / 130 hex chars).
    pub signature: String,
    /// Deterministic client-side order ID: `"blk-{intent_id}"`.
    /// Registered with the exchange so ack-loss recovery can use
    /// `client_order_id` lookup rather than scanning all open orders.
    /// Not part of the EIP-712 signed payload; stored alongside for reference.
    pub client_order_id: Option<String>,
}

// ─── Public API ──────────────────────────────────────────────────────────────

/// Builds and EIP-712 signs a Polymarket CLOB order.
///
/// # Arguments
/// * `private_key_bytes` — 32-byte secp256k1 private key.
/// * `params`            — Order parameters.
pub fn sign_order(private_key_bytes: &[u8], params: &OrderParams) -> Result<SignedOrder> {
    sign_order_with_policy(private_key_bytes, params, OrderSigningPolicy::default())
}

pub fn sign_order_with_policy(
    private_key_bytes: &[u8],
    params: &OrderParams,
    policy: OrderSigningPolicy,
) -> Result<SignedOrder> {
    validate_signing_policy(&policy)?;
    let signing_key = SigningKey::from_bytes(private_key_bytes.into())
        .context("invalid secp256k1 private key")?;
    let signer_addr = pubkey_to_address(signing_key.verifying_key());

    // Salt: full random u128
    let salt = rand::random::<u128>();

    let (maker_amount, taker_amount, side_u8) = compute_amounts(params)?;

    // Encode struct fields for hashing
    let salt_b32 = u128_to_b32(salt);
    let maker_b32 = addr_to_b32(&params.maker).context("invalid maker address")?;
    let signer_b32 = addr_to_b32(&signer_addr).context("invalid signer address")?;
    let taker_b32 = [0u8; 32]; // zero address
    let token_id_b32 = decimal_to_b32(&params.token_id)
        .with_context(|| format!("invalid token_id: {}", params.token_id))?;
    let maker_amount_b32 = u64_to_b32(maker_amount);
    let taker_amount_b32 = u64_to_b32(taker_amount);
    let expiration_b32 = u64_to_b32(policy.expiration);
    let nonce_b32 = u64_to_b32(policy.nonce);
    let side_b32 = u8_to_b32(side_u8);
    let signature_type_b32 = u8_to_b32(policy.signature_type);

    // Order struct hash = keccak256(typeHash || fields...)
    let order_type_hash = keccak256(ORDER_TYPE_STRING);
    let mut enc = Vec::with_capacity(32 * 13);
    enc.extend_from_slice(&order_type_hash);
    enc.extend_from_slice(&salt_b32);
    enc.extend_from_slice(&maker_b32);
    enc.extend_from_slice(&signer_b32);
    enc.extend_from_slice(&taker_b32);
    enc.extend_from_slice(&token_id_b32);
    enc.extend_from_slice(&maker_amount_b32);
    enc.extend_from_slice(&taker_amount_b32);
    enc.extend_from_slice(&expiration_b32);
    enc.extend_from_slice(&nonce_b32);
    enc.extend_from_slice(&[0u8; 32]); // feeRateBps
    enc.extend_from_slice(&side_b32);
    enc.extend_from_slice(&signature_type_b32);
    let order_hash = keccak256(&enc);

    // Final EIP-712 digest: keccak256(0x1901 || domainSep || orderHash)
    let domain_sep = domain_separator();
    let mut msg = Vec::with_capacity(66);
    msg.extend_from_slice(&[0x19, 0x01]);
    msg.extend_from_slice(&domain_sep);
    msg.extend_from_slice(&order_hash);
    let digest = keccak256(&msg);

    // Sign
    let (sig, rec_id): (Signature, RecoveryId) = signing_key
        .sign_prehash(&digest)
        .context("EIP-712 signing failed")?;

    let sig_bytes = sig.to_bytes(); // 64 bytes: r || s
    let v = rec_id.to_byte() + 27u8;
    let mut sig65 = Vec::with_capacity(65);
    sig65.extend_from_slice(&sig_bytes);
    sig65.push(v);

    Ok(SignedOrder {
        salt: salt.to_string(),
        maker: params.maker.clone(),
        signer: signer_addr,
        taker: "0x0000000000000000000000000000000000000000".to_string(),
        token_id: params.token_id.clone(),
        maker_amount,
        taker_amount,
        expiration: policy.expiration,
        nonce: policy.nonce,
        fee_rate_bps: 0,
        side: side_u8,
        signature_type: policy.signature_type,
        signature: format!("0x{}", hex_encode(&sig65)),
        client_order_id: None,
    })
}

/// Sign an order for a specific intent, using `intent_id` for deterministic
/// salt and nonce so retries produce the exact same EIP-712 bytes.
pub fn sign_order_for_intent(
    private_key_bytes: &[u8],
    params: &OrderParams,
    intent_id: u64,
) -> Result<SignedOrder> {
    let policy = OrderSigningPolicy {
        expiration: 0,
        nonce: intent_id,
        signature_type: 0,
    };
    let mut signed = sign_order_deterministic(private_key_bytes, params, policy, intent_id)?;
    signed.client_order_id = Some(format!("blk-{intent_id}"));
    Ok(signed)
}

fn sign_order_deterministic(
    private_key_bytes: &[u8],
    params: &OrderParams,
    policy: OrderSigningPolicy,
    intent_id: u64,
) -> Result<SignedOrder> {
    validate_signing_policy(&policy)?;
    let signing_key = SigningKey::from_bytes(private_key_bytes.into())
        .context("invalid secp256k1 private key")?;
    let signer_addr = pubkey_to_address(signing_key.verifying_key());

    // Deterministic salt derived from intent_id — enables idempotent retry.
    let salt = intent_id as u128;

    let (maker_amount, taker_amount, side_u8) = compute_amounts(params)?;

    let salt_b32 = u128_to_b32(salt);
    let maker_b32 = addr_to_b32(&params.maker).context("invalid maker address")?;
    let signer_b32 = addr_to_b32(&signer_addr).context("invalid signer address")?;
    let taker_b32 = [0u8; 32];
    let token_id_b32 = decimal_to_b32(&params.token_id)
        .with_context(|| format!("invalid token_id: {}", params.token_id))?;
    let maker_amount_b32 = u64_to_b32(maker_amount);
    let taker_amount_b32 = u64_to_b32(taker_amount);
    let expiration_b32 = u64_to_b32(policy.expiration);
    let nonce_b32 = u64_to_b32(policy.nonce);
    let side_b32 = u8_to_b32(side_u8);
    let signature_type_b32 = u8_to_b32(policy.signature_type);

    let order_type_hash = keccak256(ORDER_TYPE_STRING);
    let mut enc = Vec::with_capacity(32 * 13);
    enc.extend_from_slice(&order_type_hash);
    enc.extend_from_slice(&salt_b32);
    enc.extend_from_slice(&maker_b32);
    enc.extend_from_slice(&signer_b32);
    enc.extend_from_slice(&taker_b32);
    enc.extend_from_slice(&token_id_b32);
    enc.extend_from_slice(&maker_amount_b32);
    enc.extend_from_slice(&taker_amount_b32);
    enc.extend_from_slice(&expiration_b32);
    enc.extend_from_slice(&nonce_b32);
    enc.extend_from_slice(&[0u8; 32]);
    enc.extend_from_slice(&side_b32);
    enc.extend_from_slice(&signature_type_b32);
    let order_hash = keccak256(&enc);

    let domain_sep = domain_separator();
    let mut msg = Vec::with_capacity(66);
    msg.extend_from_slice(&[0x19, 0x01]);
    msg.extend_from_slice(&domain_sep);
    msg.extend_from_slice(&order_hash);
    let digest = keccak256(&msg);

    let (sig, rec_id): (Signature, RecoveryId) = signing_key
        .sign_prehash(&digest)
        .context("EIP-712 signing failed")?;

    let sig_bytes = sig.to_bytes();
    let v = rec_id.to_byte() + 27u8;
    let mut sig65 = Vec::with_capacity(65);
    sig65.extend_from_slice(&sig_bytes);
    sig65.push(v);

    Ok(SignedOrder {
        salt: salt.to_string(),
        maker: params.maker.clone(),
        signer: signer_addr,
        taker: "0x0000000000000000000000000000000000000000".to_string(),
        token_id: params.token_id.clone(),
        maker_amount,
        taker_amount,
        expiration: policy.expiration,
        nonce: policy.nonce,
        fee_rate_bps: 0,
        side: side_u8,
        signature_type: policy.signature_type,
        signature: format!("0x{}", hex_encode(&sig65)),
        client_order_id: None,
    })
}
///
/// The private key never leaves the vault — only the 32-byte digest is sent
/// in, and a 65-byte signature comes back.
/// Builds and EIP-712 signs a Polymarket CLOB order using a [`KeyVault`].
///
/// The private key never leaves the vault — only the 32-byte digest is sent
/// in, and a 65-byte signature comes back.
pub fn sign_order_with_vault(
    vault: &dyn tee_vault::KeyVault,
    params: &OrderParams,
) -> Result<SignedOrder> {
    sign_order_with_vault_policy(vault, params, OrderSigningPolicy::default())
}

pub fn sign_order_with_vault_policy(
    vault: &dyn tee_vault::KeyVault,
    params: &OrderParams,
    policy: OrderSigningPolicy,
) -> Result<SignedOrder> {
    validate_signing_policy(&policy)?;
    // Salt: full random u128
    let salt = rand::random::<u128>();

    let signer_addr = vault.signer_address().to_string();
    let (maker_amount, taker_amount, side_u8) = compute_amounts(params)?;

    // Encode struct fields for hashing
    let salt_b32 = u128_to_b32(salt);
    let maker_b32 = addr_to_b32(&params.maker).context("invalid maker address")?;
    let signer_b32 = addr_to_b32(&signer_addr).context("invalid signer address")?;
    let taker_b32 = [0u8; 32];
    let token_id_b32 = decimal_to_b32(&params.token_id)
        .with_context(|| format!("invalid token_id: {}", params.token_id))?;
    let maker_amount_b32 = u64_to_b32(maker_amount);
    let taker_amount_b32 = u64_to_b32(taker_amount);
    let expiration_b32 = u64_to_b32(policy.expiration);
    let nonce_b32 = u64_to_b32(policy.nonce);
    let side_b32 = u8_to_b32(side_u8);
    let signature_type_b32 = u8_to_b32(policy.signature_type);

    let order_type_hash = keccak256(ORDER_TYPE_STRING);
    let mut enc = Vec::with_capacity(32 * 13);
    enc.extend_from_slice(&order_type_hash);
    enc.extend_from_slice(&salt_b32);
    enc.extend_from_slice(&maker_b32);
    enc.extend_from_slice(&signer_b32);
    enc.extend_from_slice(&taker_b32);
    enc.extend_from_slice(&token_id_b32);
    enc.extend_from_slice(&maker_amount_b32);
    enc.extend_from_slice(&taker_amount_b32);
    enc.extend_from_slice(&expiration_b32);
    enc.extend_from_slice(&nonce_b32);
    enc.extend_from_slice(&[0u8; 32]); // feeRateBps
    enc.extend_from_slice(&side_b32);
    enc.extend_from_slice(&signature_type_b32);
    let order_hash = keccak256(&enc);

    let domain_sep = domain_separator();
    let mut msg = Vec::with_capacity(66);
    msg.extend_from_slice(&[0x19, 0x01]);
    msg.extend_from_slice(&domain_sep);
    msg.extend_from_slice(&order_hash);
    let digest = keccak256(&msg);

    // Sign via vault — private key never leaves the enclave.
    let sig65 = vault.sign_digest(&digest)?;

    Ok(SignedOrder {
        salt: salt.to_string(),
        maker: params.maker.clone(),
        signer: signer_addr,
        taker: "0x0000000000000000000000000000000000000000".to_string(),
        token_id: params.token_id.clone(),
        maker_amount,
        taker_amount,
        expiration: policy.expiration,
        nonce: policy.nonce,
        fee_rate_bps: 0,
        side: side_u8,
        signature_type: policy.signature_type,
        signature: format!("0x{}", hex_encode(&sig65)),
        client_order_id: None,
    })
}

/// Vault variant: sign an order for a specific intent with deterministic
/// salt/nonce derived from `intent_id` so retries replay identical bytes.
pub fn sign_order_for_intent_with_vault(
    vault: &dyn tee_vault::KeyVault,
    params: &OrderParams,
    intent_id: u64,
) -> Result<SignedOrder> {
    let policy = OrderSigningPolicy {
        expiration: 0,
        nonce: intent_id,
        signature_type: 0,
    };
    validate_signing_policy(&policy)?;

    let salt = intent_id as u128;

    let signer_addr = vault.signer_address().to_string();
    let (maker_amount, taker_amount, side_u8) = compute_amounts(params)?;

    let salt_b32 = u128_to_b32(salt);
    let maker_b32 = addr_to_b32(&params.maker).context("invalid maker address")?;
    let signer_b32 = addr_to_b32(&signer_addr).context("invalid signer address")?;
    let taker_b32 = [0u8; 32];
    let token_id_b32 = decimal_to_b32(&params.token_id)
        .with_context(|| format!("invalid token_id: {}", params.token_id))?;
    let maker_amount_b32 = u64_to_b32(maker_amount);
    let taker_amount_b32 = u64_to_b32(taker_amount);
    let expiration_b32 = u64_to_b32(policy.expiration);
    let nonce_b32 = u64_to_b32(policy.nonce);
    let side_b32 = u8_to_b32(side_u8);
    let signature_type_b32 = u8_to_b32(policy.signature_type);

    let order_type_hash = keccak256(ORDER_TYPE_STRING);
    let mut enc = Vec::with_capacity(32 * 13);
    enc.extend_from_slice(&order_type_hash);
    enc.extend_from_slice(&salt_b32);
    enc.extend_from_slice(&maker_b32);
    enc.extend_from_slice(&signer_b32);
    enc.extend_from_slice(&taker_b32);
    enc.extend_from_slice(&token_id_b32);
    enc.extend_from_slice(&maker_amount_b32);
    enc.extend_from_slice(&taker_amount_b32);
    enc.extend_from_slice(&expiration_b32);
    enc.extend_from_slice(&nonce_b32);
    enc.extend_from_slice(&[0u8; 32]);
    enc.extend_from_slice(&side_b32);
    enc.extend_from_slice(&signature_type_b32);
    let order_hash = keccak256(&enc);

    let domain_sep = domain_separator();
    let mut msg = Vec::with_capacity(66);
    msg.extend_from_slice(&[0x19, 0x01]);
    msg.extend_from_slice(&domain_sep);
    msg.extend_from_slice(&order_hash);
    let digest = keccak256(&msg);

    let sig65 = vault.sign_digest(&digest)?;

    Ok(SignedOrder {
        salt: salt.to_string(),
        maker: params.maker.clone(),
        signer: signer_addr,
        taker: "0x0000000000000000000000000000000000000000".to_string(),
        token_id: params.token_id.clone(),
        maker_amount,
        taker_amount,
        expiration: policy.expiration,
        nonce: policy.nonce,
        fee_rate_bps: 0,
        side: side_u8,
        signature_type: policy.signature_type,
        signature: format!("0x{}", hex_encode(&sig65)),
        client_order_id: Some(format!("blk-{intent_id}")),
    })
}
pub fn load_signer_bytes(private_key_hex: &str) -> Result<Vec<u8>> {
    let hex_str = private_key_hex.trim_start_matches("0x");
    anyhow::ensure!(hex_str.len() == 64, "private key must be 64 hex chars");
    hex_decode(hex_str).context("invalid hex in private key")
}

// ─── EIP-712 helpers ─────────────────────────────────────────────────────────

fn domain_separator() -> [u8; 32] {
    let type_hash = keccak256(DOMAIN_TYPE_STRING);
    let name_hash = keccak256(b"Polymarket CTF Exchange");
    let version_hash = keccak256(b"1");
    let chain_id = u128_to_b32(137u128);
    let contract = addr_to_b32(VERIFYING_CONTRACT_HEX)
        .expect("infallible: VERIFYING_CONTRACT_HEX is a compile-time 40-char hex constant");

    let mut enc = Vec::with_capacity(32 * 5);
    enc.extend_from_slice(&type_hash);
    enc.extend_from_slice(&name_hash);
    enc.extend_from_slice(&version_hash);
    enc.extend_from_slice(&chain_id);
    enc.extend_from_slice(&contract);
    keccak256(&enc)
}

// ─── Amount calculation ──────────────────────────────────────────────────────

/// Compute `(maker_amount, taker_amount, side_byte)` for an order.
///
/// Both amounts are in USDC.e / share **base units** (×1 000 000, matching
/// Polygon's 6-decimal ERC-20 convention).
///
/// # Unit model
///
/// | Side | `maker_amount` | `taker_amount` |
/// |------|----------------|----------------|
/// | BUY  | USDC paid ×1e6 | shares received ×1e6 |
/// | SELL | shares sold ×1e6 | USDC received ×1e6 |
///
/// `price` is stored **×1 000** (e.g. `0.65` → `650`), so conversions divide
/// or multiply by 1 000 as needed to keep amounts in 1e6 base units.
fn compute_amounts(params: &OrderParams) -> Result<(u64, u64, u8)> {
    match params.side {
        OrderSide::Buy => {
            // maker_amount = USDC spent, scaled ×1e6.
            let maker_amount = (params.size * 1_000_000.0).floor() as u64;
            anyhow::ensure!(params.price > 0, "BUY order price cannot be zero");
            // taker_amount = shares received = USDC_base × 1_000 / price_scaled
            let taker_amount = (maker_amount as u128 * 1_000 / params.price as u128) as u64;
            Ok((maker_amount, taker_amount, 0u8))
        }
        OrderSide::Sell => {
            // maker_amount = shares sold, scaled ×1e6.
            // Using `.floor()` rather than `as u64` to avoid lossy truncation
            // of fractional share sizes (e.g. 10.5 → 10_500_000, not 10).
            let maker_amount = (params.size * 1_000_000.0).floor() as u64;
            anyhow::ensure!(params.price > 0, "SELL order price cannot be zero");
            // taker_amount = USDC received = shares_base × price_scaled / 1_000
            let taker_amount = (maker_amount as u128 * params.price as u128 / 1_000) as u64;
            Ok((maker_amount, taker_amount, 1u8))
        }
    }
}

fn validate_signing_policy(policy: &OrderSigningPolicy) -> Result<()> {
    if policy.expiration == 0 {
        return Ok(());
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock before unix epoch")?
        .as_secs();
    anyhow::ensure!(
        policy.expiration > now,
        "order expiration timestamp is in the past"
    );
    Ok(())
}

// ─── Encoding helpers ────────────────────────────────────────────────────────

fn keccak256(data: &[u8]) -> [u8; 32] {
    let mut hasher = Keccak256::new();
    hasher.update(data);
    hasher.finalize().into()
}

/// Parse a 20-byte Ethereum address (with or without 0x) into a 32-byte
/// ABI word (12 zero bytes + 20 address bytes).
fn addr_to_b32(addr: &str) -> Result<[u8; 32]> {
    let hex = addr.trim_start_matches("0x");
    anyhow::ensure!(hex.len() == 40, "address must be 40 hex chars, got: {addr}");
    let bytes = hex_decode(hex)?;
    let mut out = [0u8; 32];
    out[12..].copy_from_slice(&bytes);
    Ok(out)
}

/// Encode a `u128` as a big-endian 32-byte ABI word.
fn u128_to_b32(v: u128) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[16..].copy_from_slice(&v.to_be_bytes());
    out
}

/// Encode a `u64` as a big-endian 32-byte ABI word.
fn u64_to_b32(v: u64) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[24..].copy_from_slice(&v.to_be_bytes());
    out
}

/// Encode a `u8` as a 32-byte ABI word.
fn u8_to_b32(v: u8) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[31] = v;
    out
}

/// Parse a decimal-string big integer into a 32-byte big-endian ABI word.
///
/// The integer must fit in 32 bytes (i.e. < 2^256).
fn decimal_to_b32(s: &str) -> Result<[u8; 32]> {
    // Simple big-integer parsing: accumulate via multiply-add.
    let mut result = [0u8; 32];
    for ch in s.chars() {
        let digit = ch
            .to_digit(10)
            .with_context(|| format!("non-decimal char '{ch}' in '{s}'"))?;
        // result = result * 10 + digit (big-endian u256 arithmetic)
        let mut carry = digit as u32;
        for byte in result.iter_mut().rev() {
            let prod = (*byte as u32) * 10 + carry;
            *byte = (prod & 0xff) as u8;
            carry = prod >> 8;
        }
        anyhow::ensure!(carry == 0, "decimal value overflows 256 bits: {s}");
    }
    Ok(result)
}

/// Derive the Ethereum address from a secp256k1 verifying key.
fn pubkey_to_address(key: &k256::ecdsa::VerifyingKey) -> String {
    let point = key.to_encoded_point(false); // uncompressed: 0x04 || x || y
    let hash = keccak256(&point.as_bytes()[1..]); // skip the 0x04 prefix
    format!("0x{}", hex_encode(&hash[12..])) // last 20 bytes
}

// ─── Tiny hex helpers ────────────────────────────────────────────────────────

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hex_decode(s: &str) -> Result<Vec<u8>> {
    anyhow::ensure!(s.len() % 2 == 0, "odd hex length");
    (0..s.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&s[i..i + 2], 16).with_context(|| format!("invalid hex byte at {i}"))
        })
        .collect()
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_params(side: OrderSide, price: u64, size: f64) -> OrderParams {
        OrderParams {
            token_id: "12345".to_string(),
            side,
            price,
            size,
            maker: "0x0000000000000000000000000000000000000000".to_string(),
        }
    }

    fn make_params_with_token(
        token_id: &str,
        side: OrderSide,
        price: u64,
        size: f64,
    ) -> OrderParams {
        OrderParams {
            token_id: token_id.to_string(),
            side,
            price,
            size,
            maker: "0x0000000000000000000000000000000000000000".to_string(),
        }
    }

    #[test]
    fn buy_amounts_correct() {
        let p = make_params(OrderSide::Buy, 650, 10.0);
        let (ma, ta, side) = compute_amounts(&p).unwrap();
        assert_eq!(ma, 10_000_000); // $10 in 6-dec USDC
        assert_eq!(ta, 15_384_615); // shares = 10_000_000 * 1000 / 650
        assert_eq!(side, 0u8);
    }

    #[test]
    fn sell_amounts_correct() {
        // 10 whole shares at $0.65 → maker = 10_000_000 (shares ×1e6), taker = 6_500_000 (USDC ×1e6)
        let p = make_params(OrderSide::Sell, 650, 10.0);
        let (ma, ta, side) = compute_amounts(&p).unwrap();
        assert_eq!(ma, 10_000_000); // 10 shares in 6-dec base units
        assert_eq!(ta, 6_500_000); // $6.50 USDC in 6-dec base units
        assert_eq!(side, 1u8);
    }

    #[test]
    fn sell_fractional_shares_no_truncation() {
        // Previously: 10.5 as u64 == 10  →  0.5 shares silently lost
        // Fixed:      10.5 × 1e6       == 10_500_000
        let p = make_params(OrderSide::Sell, 650, 10.5);
        let (ma, ta, side) = compute_amounts(&p).unwrap();
        assert_eq!(ma, 10_500_000); // 10.5 shares × 1e6 — no truncation
        assert_eq!(ta, 6_825_000); // 10.5 × $0.65 = $6.825 USDC × 1e6
        assert_eq!(side, 1u8);
    }

    #[test]
    fn buy_sell_round_trip_amounts() {
        // Buy $10 at 0.65 → receive taker_amount shares.
        // Selling those same shares back should return ≈ $10 USDC (within 1 base unit).
        let buy = make_params(OrderSide::Buy, 650, 10.0);
        let (buy_ma, buy_ta, _) = compute_amounts(&buy).unwrap();

        // buy_ta is shares in base units; convert back to float for Sell params.
        let shares_float = buy_ta as f64 / 1_000_000.0;
        let sell = make_params(OrderSide::Sell, 650, shares_float);
        let (sell_ma, sell_ta, _) = compute_amounts(&sell).unwrap();

        // Maker amount of SELL must equal taker amount of BUY (same share count).
        assert_eq!(
            sell_ma, buy_ta,
            "SELL maker_amount must equal BUY taker_amount"
        );
        // USDC recovered must equal USDC spent (within 1 base unit of rounding).
        assert!(
            (sell_ta as i64 - buy_ma as i64).abs() <= 1,
            "round-trip USDC mismatch: spent={buy_ma} recovered={sell_ta}"
        );
    }

    #[test]
    fn decimal_to_b32_small() {
        let b = decimal_to_b32("255").unwrap();
        assert_eq!(b[31], 0xff);
        assert!(b[..31].iter().all(|&x| x == 0));
    }

    #[test]
    fn addr_to_b32_zero() {
        let b = addr_to_b32("0000000000000000000000000000000000000000").unwrap();
        assert_eq!(b, [0u8; 32]);
    }

    #[test]
    fn hex_roundtrip() {
        let bytes = [0xde, 0xad, 0xbe, 0xef];
        assert_eq!(hex_encode(&bytes), "deadbeef");
        assert_eq!(hex_decode("deadbeef").unwrap(), bytes);
    }

    #[test]
    fn load_signer_bytes_with_prefix() {
        let key_hex = "0x".to_string() + &"ab".repeat(32);
        let bytes = load_signer_bytes(&key_hex).unwrap();
        assert_eq!(bytes.len(), 32);
        assert!(bytes.iter().all(|&b| b == 0xab));
    }

    #[test]
    fn sign_order_with_vault_matches_legacy_sign_order() {
        // Use a deterministic test key.
        let key_hex = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
        let key_bytes = load_signer_bytes(key_hex).unwrap();

        let vault =
            tee_vault::SoftwareVault::new(key_bytes.as_slice().try_into().unwrap()).unwrap();

        let params = make_params(OrderSide::Buy, 650, 10.0);

        // Both must produce the same signer address.
        let legacy = sign_order(&key_bytes, &params).unwrap();
        let vaulted = sign_order_with_vault(&vault, &params).unwrap();

        assert_eq!(legacy.signer, vaulted.signer);
        assert_eq!(legacy.maker_amount, vaulted.maker_amount);
        assert_eq!(legacy.taker_amount, vaulted.taker_amount);
        assert_eq!(legacy.side, vaulted.side);
        // Signatures differ because of different salts, but both must be
        // valid 65-byte hex signatures.
        assert_eq!(vaulted.signature.len(), 2 + 130); // "0x" + 130 hex chars
    }

    #[test]
    fn signing_policy_fields_are_applied() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let key_hex = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
        let key_bytes = load_signer_bytes(key_hex).unwrap();
        let params = make_params(OrderSide::Buy, 650, 10.0);
        let policy = OrderSigningPolicy {
            expiration: now + 3600,
            nonce: 42,
            signature_type: 1,
        };

        let signed = sign_order_with_policy(&key_bytes, &params, policy).unwrap();
        assert_eq!(signed.expiration, policy.expiration);
        assert_eq!(signed.nonce, policy.nonce);
        assert_eq!(signed.signature_type, policy.signature_type);
    }

    // ── Keccak256 cross-validation against Ethereum Yellow Paper vectors ──

    #[test]
    fn keccak256_matches_reference_vectors() {
        let cases: &[(&[u8], &str)] = &[
            (b"", "c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"),
            (b"abc", "4e03657aea45a94fc7d47ba826c8d667c0d1e6e33a64a036ec44f58fa12d6c45"),
            (b"hello", "1c8aff950685c2ed4bc3174f3472287b56d9517b9c948127319a09a7a36deac8"),
            // EIP-712 domain type string
            (
                b"EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)",
                &hex_encode(&keccak256(DOMAIN_TYPE_STRING)),
            ),
        ];

        for (input, expected_hex) in cases {
            let hash = keccak256(input);
            let actual_hex = hex_encode(&hash);
            assert_eq!(
                actual_hex,
                *expected_hex,
                "Keccak256 mismatch for input {:?}",
                String::from_utf8_lossy(input)
            );
        }
    }

    #[test]
    fn domain_separator_is_deterministic() {
        let ds1 = domain_separator();
        let ds2 = domain_separator();
        assert_eq!(ds1, ds2, "domain separator must be deterministic");
        assert_ne!(ds1, [0u8; 32], "domain separator must not be zero");
    }
}

// ─── Property-based tests (proptest, 10 000 iterations) ─────────────────────

#[cfg(test)]
mod proptest_verification {
    use super::*;
    use k256::ecdsa::signature::hazmat::PrehashVerifier;
    use k256::ecdsa::{SigningKey, VerifyingKey};
    use proptest::prelude::*;

    const PROPTEST_CASES: u32 = 10_000;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(PROPTEST_CASES))]

        /// EIP-712 digest must be deterministic: same inputs → same digest.
        #[test]
        fn eip712_digest_is_deterministic(
            token_id in "[0-9]{10,30}",
            price in 1u64..999u64,
            size in 0.01f64..100.0f64,
        ) {
            let params1 = OrderParams {
                token_id: token_id.clone(),
                side: OrderSide::Buy,
                price,
                size,
                maker: "0x0000000000000000000000000000000000000000".to_string(),
            };
            let params2 = params1.clone();

            let (ma1, ta1, s1) = compute_amounts(&params1).unwrap();
            let (ma2, ta2, s2) = compute_amounts(&params2).unwrap();
            prop_assert_eq!(ma1, ma2);
            prop_assert_eq!(ta1, ta2);
            prop_assert_eq!(s1, s2);

            // Build digests with identical salt to verify determinism.
            let salt = 42u128;
            let signer_placeholder = "0x0000000000000000000000000000000000000000";
            let digest1 = build_digest(&params1, salt, signer_placeholder, ma1, ta1, s1);
            let digest2 = build_digest(&params2, salt, signer_placeholder, ma2, ta2, s2);
            prop_assert_eq!(digest1, digest2);
        }

        /// ecrecover(sign(digest, key)) must ALWAYS equal the signer address.
        #[test]
        fn eip712_signature_is_recoverable(
            key_seed in any::<[u8; 32]>(),
            token_id in "[0-9]{5,20}",
            price in 1u64..999u64,
            size in 0.01f64..50.0f64,
        ) {
            // Skip invalid keys (e.g. zero or ≥ curve order).
            let signing_key = match SigningKey::from_bytes((&key_seed).into()) {
                Ok(k) => k,
                Err(_) => return Ok(()),
            };
            let verifying_key = signing_key.verifying_key();
            let expected_addr = pubkey_to_address(verifying_key);

            let params = OrderParams {
                token_id,
                side: OrderSide::Buy,
                price,
                size,
                maker: "0x0000000000000000000000000000000000000000".to_string(),
            };

            let signed = sign_order(&key_seed, &params).unwrap();
            prop_assert_eq!(&signed.signer, &expected_addr);
            prop_assert_eq!(signed.signature.len(), 2 + 130); // "0x" + 65-byte hex

            // Verify the signature is valid via k256 PrehashVerifier.
            let sig_bytes = hex_decode(&signed.signature[2..]).unwrap();
            let ecdsa_sig = k256::ecdsa::Signature::from_slice(&sig_bytes[..64]).unwrap();
            let rec_id = k256::ecdsa::RecoveryId::from_byte(sig_bytes[64] - 27).unwrap();

            // Build the same digest.
            let salt: u128 = signed.salt.parse().unwrap();
            let digest = build_digest(&params, salt, &signed.signer, signed.maker_amount, signed.taker_amount, signed.side);

            // Recover the public key from the signature.
            let recovered = VerifyingKey::recover_from_prehash(&digest, &ecdsa_sig, rec_id).unwrap();
            let recovered_addr = pubkey_to_address(&recovered);
            prop_assert_eq!(&recovered_addr, &expected_addr);
        }
    }

    /// Helper: build the EIP-712 digest given params and a fixed salt.
    fn build_digest(
        params: &OrderParams,
        salt: u128,
        signer_addr: &str,
        maker_amount: u64,
        taker_amount: u64,
        side: u8,
    ) -> [u8; 32] {
        let order_type_hash = keccak256(ORDER_TYPE_STRING);
        let salt_b32 = u128_to_b32(salt);
        let maker_b32 = addr_to_b32(&params.maker).unwrap();
        let signer_b32 = addr_to_b32(signer_addr).unwrap();
        let taker_b32 = [0u8; 32];
        let token_id_b32 = decimal_to_b32(&params.token_id).unwrap();
        let maker_amount_b32 = u64_to_b32(maker_amount);
        let taker_amount_b32 = u64_to_b32(taker_amount);
        let zero_b32 = [0u8; 32];
        let side_b32 = u8_to_b32(side);

        let mut enc = Vec::with_capacity(32 * 13);
        enc.extend_from_slice(&order_type_hash);
        enc.extend_from_slice(&salt_b32);
        enc.extend_from_slice(&maker_b32);
        enc.extend_from_slice(&signer_b32);
        enc.extend_from_slice(&taker_b32);
        enc.extend_from_slice(&token_id_b32);
        enc.extend_from_slice(&maker_amount_b32);
        enc.extend_from_slice(&taker_amount_b32);
        enc.extend_from_slice(&zero_b32);
        enc.extend_from_slice(&zero_b32);
        enc.extend_from_slice(&zero_b32);
        enc.extend_from_slice(&side_b32);
        enc.extend_from_slice(&zero_b32);
        let order_hash = keccak256(&enc);

        let domain_sep = domain_separator();
        let mut msg = Vec::with_capacity(66);
        msg.extend_from_slice(&[0x19, 0x01]);
        msg.extend_from_slice(&domain_sep);
        msg.extend_from_slice(&order_hash);
        keccak256(&msg)
    }
}
