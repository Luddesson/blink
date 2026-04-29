//! Deterministic `client_order_id` derivation.
//!
//! `coid = keccak256(intent_hash || run_id || attempt)[0..16]`.
//!
//! Deterministic-by-construction so two replays of the same intent in the
//! same run produce identical coids — the CLOB dedups the duplicate.

use blink_signer::eip712::keccak256;

/// Derive the 16-byte `client_order_id` for an intent.
///
/// * `intent_hash` — 32-byte canonical intent hash
///   (`blink_types::IntentHash.0`).
/// * `run_id` — 16-byte process / run identifier (random at process
///   start, persisted for replay).
/// * `attempt` — retry counter. Default `0`; operators bump this only
///   when they *want* a fresh coid (e.g. after reconciling a genuinely
///   lost submit).
#[inline]
pub fn derive_client_order_id(
    intent_hash: &[u8; 32],
    run_id: &[u8; 16],
    attempt: u8,
) -> [u8; 16] {
    let mut buf = [0u8; 32 + 16 + 1];
    buf[0..32].copy_from_slice(intent_hash);
    buf[32..48].copy_from_slice(run_id);
    buf[48] = attempt;
    let h = keccak256(&buf);
    let mut out = [0u8; 16];
    out.copy_from_slice(&h[0..16]);
    out
}

/// Hex-encode a coid into the 32-char lowercase string used on the wire.
#[inline]
pub fn coid_to_hex(coid: &[u8; 16]) -> String {
    let mut out = String::with_capacity(32);
    for b in coid {
        out.push(hex_nibble(b >> 4));
        out.push(hex_nibble(b & 0xf));
    }
    out
}

/// Hex-decode a 32-char coid string into 16 bytes. Returns `None` on any
/// malformed input.
pub fn coid_from_hex(s: &str) -> Option<[u8; 16]> {
    if s.len() != 32 {
        return None;
    }
    let bytes = s.as_bytes();
    let mut out = [0u8; 16];
    for i in 0..16 {
        let hi = nibble_from_hex(bytes[i * 2])?;
        let lo = nibble_from_hex(bytes[i * 2 + 1])?;
        out[i] = (hi << 4) | lo;
    }
    Some(out)
}

#[inline]
fn hex_nibble(n: u8) -> char {
    match n {
        0..=9 => (b'0' + n) as char,
        10..=15 => (b'a' + (n - 10)) as char,
        _ => unreachable!(),
    }
}

#[inline]
fn nibble_from_hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic() {
        let ih = [7u8; 32];
        let run = [3u8; 16];
        let a = derive_client_order_id(&ih, &run, 0);
        let b = derive_client_order_id(&ih, &run, 0);
        assert_eq!(a, b);
    }

    #[test]
    fn attempt_changes_coid() {
        let ih = [7u8; 32];
        let run = [3u8; 16];
        let a = derive_client_order_id(&ih, &run, 0);
        let b = derive_client_order_id(&ih, &run, 1);
        assert_ne!(a, b);
    }

    #[test]
    fn run_id_changes_coid() {
        let ih = [7u8; 32];
        let a = derive_client_order_id(&ih, &[3u8; 16], 0);
        let b = derive_client_order_id(&ih, &[4u8; 16], 0);
        assert_ne!(a, b);
    }

    #[test]
    fn hex_roundtrip() {
        let ih = [9u8; 32];
        let run = [2u8; 16];
        let coid = derive_client_order_id(&ih, &run, 7);
        let s = coid_to_hex(&coid);
        assert_eq!(s.len(), 32);
        assert_eq!(coid_from_hex(&s), Some(coid));
    }

    #[test]
    fn hex_rejects_bad_input() {
        assert_eq!(coid_from_hex("short"), None);
        assert_eq!(coid_from_hex(&"z".repeat(32)), None);
    }
}
