//! Polymarket L2 HMAC auth header builder.
//!
//! Mirrors `engine/src/order_executor.rs::build_auth_headers`.
// WIRE FORMAT REF: engine/src/order_executor.rs:790-825

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::time::{SystemTime, UNIX_EPOCH};

type HmacSha256 = Hmac<Sha256>;

/// Opaque L2 credentials. `api_secret_b64` is the exchange-provided
/// base64-encoded HMAC secret (decoded on every request).
#[derive(Clone)]
pub struct PolyAuthCreds {
    pub api_key: String,
    pub api_secret_b64: String,
    pub passphrase: String,
    /// Maker (proxy) address, lowercase hex `"0x..."`.
    pub maker_address: String,
}

/// Errors raised while building auth headers.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("system clock before unix epoch")]
    Clock,
    #[error("POLY_API_SECRET is not valid base64")]
    BadSecret,
    #[error("HMAC init failed")]
    HmacInit,
}

/// Build the six Polymarket auth headers for a single request.
///
/// `message = timestamp + method + path + body`.
pub fn build_poly_headers(
    creds: &PolyAuthCreds,
    method: &str,
    path: &str,
    body: &str,
) -> Result<[(String, String); 7], AuthError> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| AuthError::Clock)?
        .as_secs()
        .to_string();

    let message = format!("{timestamp}{method}{path}{body}");
    let secret = B64
        .decode(&creds.api_secret_b64)
        .map_err(|_| AuthError::BadSecret)?;

    let mut mac = HmacSha256::new_from_slice(&secret).map_err(|_| AuthError::HmacInit)?;
    mac.update(message.as_bytes());
    let sig_b64 = B64.encode(mac.finalize().into_bytes());

    Ok([
        ("POLY-ADDRESS".to_string(), creds.maker_address.clone()),
        ("POLY-SIGNATURE".to_string(), sig_b64),
        ("POLY-TIMESTAMP".to_string(), timestamp),
        ("POLY-NONCE".to_string(), "0".to_string()),
        ("POLY-API-KEY".to_string(), creds.api_key.clone()),
        ("POLY-PASSPHRASE".to_string(), creds.passphrase.clone()),
        ("content-type".to_string(), "application/json".to_string()),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::STANDARD as B64, Engine};

    #[test]
    fn builds_six_plus_ct_headers() {
        let creds = PolyAuthCreds {
            api_key: "k".into(),
            api_secret_b64: B64.encode(b"secret"),
            passphrase: "p".into(),
            maker_address: "0x0000000000000000000000000000000000000000".into(),
        };
        let hs = build_poly_headers(&creds, "POST", "/order", "{}").unwrap();
        let names: Vec<&str> = hs.iter().map(|(k, _)| k.as_str()).collect();
        assert!(names.contains(&"POLY-ADDRESS"));
        assert!(names.contains(&"POLY-SIGNATURE"));
        assert!(names.contains(&"POLY-TIMESTAMP"));
        assert!(names.contains(&"POLY-NONCE"));
        assert!(names.contains(&"POLY-API-KEY"));
        assert!(names.contains(&"POLY-PASSPHRASE"));
        assert!(names.contains(&"content-type"));
    }

    #[test]
    fn bad_secret_rejected() {
        let creds = PolyAuthCreds {
            api_key: "k".into(),
            api_secret_b64: "!!!not base64!!!".into(),
            passphrase: "p".into(),
            maker_address: "0x".into(),
        };
        assert!(matches!(
            build_poly_headers(&creds, "POST", "/order", "{}"),
            Err(AuthError::BadSecret)
        ));
    }
}
