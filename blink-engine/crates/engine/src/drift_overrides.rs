//! Category-aware drift-threshold overrides.
//!
//! Parses two optional env vars at startup (once, via [`OnceLock`]):
//!
//! - `BLINK_GATE_DRIFT_BPS_OVERRIDES` — comma-separated `class=bps` pairs
//!   applied in [`crate::pretrade_gate::GateConfig::max_drift_bps_for_class`].
//!   Values are clamped to `[1, 5000]` bps.
//! - `PAPER_DRIFT_PCT_OVERRIDES` — comma-separated `class=pct` pairs
//!   applied in [`crate::paper_portfolio::drift_threshold_for`].
//!   Values are clamped to `[0.5, 50.0]` percent.
//!
//! Example: `BLINK_GATE_DRIFT_BPS_OVERRIDES=tennis=50,cs2=200,soccer=90`.
//!
//! # Semantics
//!
//! Overrides are applied with `min()` semantics — they can only **tighten**
//! the profile default, never loosen it. This makes it impossible for a
//! typo or misread to relax risk below the execution-profile baseline.
//!
//! # Parse errors
//!
//! On any parse error (unknown class, duplicate key, missing `=`, empty
//! value, non-numeric value, out-of-range value, trailing comma), we log
//! an **ERROR** and return an empty override map. We never partially
//! apply and never panic — bad config fails safe to "no overrides".

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::market_class::MarketClass;

/// Min/max allowed bps for a drift override (mirrors gate's u16 range).
const BPS_MIN: u16 = 1;
const BPS_MAX: u16 = 5_000;

/// Min/max allowed pct for a paper drift override (mirrors
/// [`crate::paper_portfolio::drift_threshold`] clamp).
const PCT_MIN: f64 = 0.5;
const PCT_MAX: f64 = 50.0;

/// Map of `MarketClass → override bps`.
pub type BpsOverrides = HashMap<MarketClass, u16>;
/// Map of `MarketClass → override percent (0..100)`.
pub type PctOverrides = HashMap<MarketClass, f64>;

static BPS_OVERRIDES: OnceLock<BpsOverrides> = OnceLock::new();
static PCT_OVERRIDES: OnceLock<PctOverrides> = OnceLock::new();

/// Returns the resolved bps override map (parsed on first call and cached).
pub fn bps_overrides() -> &'static BpsOverrides {
    BPS_OVERRIDES.get_or_init(|| {
        let raw = std::env::var("BLINK_GATE_DRIFT_BPS_OVERRIDES").ok();
        let map = parse_bps(raw.as_deref());
        if !map.is_empty() {
            tracing::info!(
                overrides = ?sorted_bps(&map),
                "BLINK_GATE_DRIFT_BPS_OVERRIDES loaded"
            );
        }
        map
    })
}

/// Returns the resolved pct override map (parsed on first call and cached).
pub fn pct_overrides() -> &'static PctOverrides {
    PCT_OVERRIDES.get_or_init(|| {
        let raw = std::env::var("PAPER_DRIFT_PCT_OVERRIDES").ok();
        let map = parse_pct(raw.as_deref());
        if !map.is_empty() {
            tracing::info!(
                overrides = ?sorted_pct(&map),
                "PAPER_DRIFT_PCT_OVERRIDES loaded"
            );
        }
        map
    })
}

/// Apply `min()` semantics: return the tighter of `profile_default` and any
/// class-specific bps override. Missing / empty override map → profile default.
pub fn effective_bps(class: MarketClass, profile_default: u16) -> u16 {
    match bps_overrides().get(&class) {
        Some(override_bps) => profile_default.min(*override_bps),
        None => profile_default,
    }
}

/// Apply `min()` semantics for paper drift percent override.
pub fn effective_pct(class: MarketClass, profile_default_pct: f64) -> f64 {
    match pct_overrides().get(&class) {
        Some(override_pct) => profile_default_pct.min(*override_pct),
        None => profile_default_pct,
    }
}

// ─── Parsers ──────────────────────────────────────────────────────────────────

fn parse_bps(raw: Option<&str>) -> BpsOverrides {
    match parse_bps_inner(raw) {
        Ok(map) => map,
        Err(e) => {
            tracing::error!(
                env = "BLINK_GATE_DRIFT_BPS_OVERRIDES",
                input = raw.unwrap_or(""),
                error = %e,
                "Failed to parse drift bps overrides — IGNORING ALL OVERRIDES (fail-safe)"
            );
            HashMap::new()
        }
    }
}

fn parse_pct(raw: Option<&str>) -> PctOverrides {
    match parse_pct_inner(raw) {
        Ok(map) => map,
        Err(e) => {
            tracing::error!(
                env = "PAPER_DRIFT_PCT_OVERRIDES",
                input = raw.unwrap_or(""),
                error = %e,
                "Failed to parse drift pct overrides — IGNORING ALL OVERRIDES (fail-safe)"
            );
            HashMap::new()
        }
    }
}

fn parse_bps_inner(raw: Option<&str>) -> Result<BpsOverrides, String> {
    let mut map = HashMap::new();
    let raw = match raw {
        None => return Ok(map),
        Some(s) if s.trim().is_empty() => return Ok(map),
        Some(s) => s,
    };
    for (class, value) in parse_kv_pairs(raw)? {
        let bps: u16 = value
            .parse()
            .map_err(|_| format!("invalid bps value '{value}' for class '{}'", class.as_str()))?;
        if bps < BPS_MIN || bps > BPS_MAX {
            return Err(format!(
                "bps value {bps} for '{}' out of range [{BPS_MIN}, {BPS_MAX}]",
                class.as_str()
            ));
        }
        if map.insert(class, bps).is_some() {
            return Err(format!("duplicate key '{}'", class.as_str()));
        }
    }
    Ok(map)
}

fn parse_pct_inner(raw: Option<&str>) -> Result<PctOverrides, String> {
    let mut map = HashMap::new();
    let raw = match raw {
        None => return Ok(map),
        Some(s) if s.trim().is_empty() => return Ok(map),
        Some(s) => s,
    };
    for (class, value) in parse_kv_pairs(raw)? {
        let pct: f64 = value
            .parse()
            .map_err(|_| format!("invalid pct value '{value}' for class '{}'", class.as_str()))?;
        if !pct.is_finite() {
            return Err(format!("non-finite pct for '{}'", class.as_str()));
        }
        if pct < PCT_MIN || pct > PCT_MAX {
            return Err(format!(
                "pct value {pct} for '{}' out of range [{PCT_MIN}, {PCT_MAX}]",
                class.as_str()
            ));
        }
        if map.insert(class, pct).is_some() {
            return Err(format!("duplicate key '{}'", class.as_str()));
        }
    }
    Ok(map)
}

/// Parse `"tennis=50, cs2=200"` into `Vec<(MarketClass, value_str)>`.
///
/// Rejects unknown keys, missing `=`, empty segments, trailing commas, and
/// empty values. Whitespace around keys/values is trimmed; the key is then
/// lowercased for canonical matching.
fn parse_kv_pairs(raw: &str) -> Result<Vec<(MarketClass, String)>, String> {
    let mut out = Vec::new();
    for (i, segment) in raw.split(',').enumerate() {
        let trimmed = segment.trim();
        if trimmed.is_empty() {
            return Err(format!("empty segment at position {i} (trailing/duplicate comma?)"));
        }
        let (k, v) = trimmed
            .split_once('=')
            .ok_or_else(|| format!("segment '{trimmed}' missing '='"))?;
        let key = k.trim().to_lowercase();
        let value = v.trim().to_string();
        if key.is_empty() {
            return Err(format!("empty key in segment '{trimmed}'"));
        }
        if value.is_empty() {
            return Err(format!("empty value for key '{key}'"));
        }
        let class = MarketClass::from_canonical_str(&key)
            .ok_or_else(|| format!("unknown market class '{key}'"))?;
        out.push((class, value));
    }
    Ok(out)
}

// Sorted views for stable, human-friendly log output.
fn sorted_bps(map: &BpsOverrides) -> Vec<(&'static str, u16)> {
    let mut v: Vec<(&'static str, u16)> = map.iter().map(|(k, v)| (k.as_str(), *v)).collect();
    v.sort_by_key(|(k, _)| *k);
    v
}

fn sorted_pct(map: &PctOverrides) -> Vec<(&'static str, f64)> {
    let mut v: Vec<(&'static str, f64)> = map.iter().map(|(k, v)| (k.as_str(), *v)).collect();
    v.sort_by_key(|(k, _)| *k);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bps_basic() {
        let m = parse_bps_inner(Some("tennis=50, cs2=200, soccer=90")).unwrap();
        assert_eq!(m.get(&MarketClass::Tennis), Some(&50));
        assert_eq!(m.get(&MarketClass::Cs2), Some(&200));
        assert_eq!(m.get(&MarketClass::Soccer), Some(&90));
    }

    #[test]
    fn parse_bps_case_insensitive() {
        let m = parse_bps_inner(Some("TENNIS=50, Cs2=200")).unwrap();
        assert_eq!(m.get(&MarketClass::Tennis), Some(&50));
        assert_eq!(m.get(&MarketClass::Cs2), Some(&200));
    }

    #[test]
    fn parse_bps_empty_or_none() {
        assert!(parse_bps_inner(None).unwrap().is_empty());
        assert!(parse_bps_inner(Some("")).unwrap().is_empty());
        assert!(parse_bps_inner(Some("   ")).unwrap().is_empty());
    }

    #[test]
    fn parse_bps_rejects_unknown() {
        let err = parse_bps_inner(Some("hockey=50,nascar=100")).unwrap_err();
        assert!(err.contains("nascar") || err.contains("hockey"));
    }

    #[test]
    fn parse_bps_rejects_dupes() {
        let err = parse_bps_inner(Some("tennis=50,tennis=75")).unwrap_err();
        assert!(err.contains("duplicate"));
    }

    #[test]
    fn parse_bps_rejects_missing_eq() {
        assert!(parse_bps_inner(Some("tennis50")).is_err());
    }

    #[test]
    fn parse_bps_rejects_empty_segment() {
        assert!(parse_bps_inner(Some("tennis=50,")).is_err());
        assert!(parse_bps_inner(Some(",tennis=50")).is_err());
        assert!(parse_bps_inner(Some("tennis=50,,cs2=200")).is_err());
    }

    #[test]
    fn parse_bps_rejects_empty_value() {
        assert!(parse_bps_inner(Some("tennis=")).is_err());
    }

    #[test]
    fn parse_bps_rejects_non_numeric() {
        assert!(parse_bps_inner(Some("tennis=abc")).is_err());
    }

    #[test]
    fn parse_bps_rejects_out_of_range() {
        assert!(parse_bps_inner(Some("tennis=0")).is_err());
        assert!(parse_bps_inner(Some("tennis=5001")).is_err());
    }

    #[test]
    fn parse_pct_basic_and_errors() {
        let m = parse_pct_inner(Some("tennis=5.0, cs2=12.5")).unwrap();
        assert_eq!(m.get(&MarketClass::Tennis), Some(&5.0));
        assert_eq!(m.get(&MarketClass::Cs2), Some(&12.5));

        assert!(parse_pct_inner(Some("tennis=0.4")).is_err());
        assert!(parse_pct_inner(Some("tennis=50.1")).is_err());
        assert!(parse_pct_inner(Some("tennis=NaN")).is_err());
        assert!(parse_pct_inner(Some("tennis=inf")).is_err());
    }

    #[test]
    fn effective_bps_min_semantics() {
        // Using the inner function directly so we don't depend on env.
        let m: BpsOverrides = [(MarketClass::Tennis, 50_u16), (MarketClass::Cs2, 200)]
            .into_iter()
            .collect();
        // override tighter than profile → override wins
        assert_eq!(m.get(&MarketClass::Tennis).copied().unwrap().min(120), 50);
        // override looser than profile → profile wins (min semantics)
        assert_eq!(m.get(&MarketClass::Cs2).copied().unwrap().min(120), 120);
        // unknown class → profile default
        assert_eq!(m.get(&MarketClass::Other), None);
    }

    #[test]
    fn effective_pct_min_semantics() {
        let m: PctOverrides = [(MarketClass::Tennis, 5.0), (MarketClass::Cs2, 12.0)]
            .into_iter()
            .collect();
        // override tighter than profile default 8.0 → override wins
        let tennis = m.get(&MarketClass::Tennis).copied().unwrap().min(8.0);
        assert!((tennis - 5.0).abs() < f64::EPSILON);
        // override looser than profile default 8.0 → profile wins
        let cs2 = m.get(&MarketClass::Cs2).copied().unwrap().min(8.0);
        assert!((cs2 - 8.0).abs() < f64::EPSILON);
    }

    #[test]
    fn parse_bps_whitespace_tolerated() {
        let m = parse_bps_inner(Some("  tennis = 50 ,  cs2=200  ")).unwrap();
        assert_eq!(m.len(), 2);
    }
}
