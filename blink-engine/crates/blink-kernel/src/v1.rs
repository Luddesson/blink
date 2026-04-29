//! V1 reference decision kernel. Ports
//! `engine::paper_engine::handle_signal` (pretrade gate + drift / stale
//! / post-only / cooldown / risk / edge / dedup) to a pure, borrowed,
//! allocation-free implementation.
//!
//! Legacy refs (read-only):
//!
//! - Stale / drift / post-only: `crates/engine/src/pretrade_gate.rs`
//!   (`GateDecision::{SkipStale, SkipDrift, SkipPostOnlyCross}`) called
//!   from `paper_engine.rs:1725` inside `handle_signal` (starts line
//!   592).
//! - Drift-abort cooldown map: `paper_engine.rs:1696..1709`
//!   (`drift_abort_cooldown`).
//! - Risk + inventory gate: `paper_engine.rs:1830..` (post-abort flow).
//!
//! The ordering matches legacy: stale → drift → post-only cross →
//! cooldown → risk → edge → dedup → submit.

use crate::{
    config::KernelConfig,
    kernel::{DecisionKernel, KernelVerdict},
    snapshot::DecisionSnapshot,
    stats::KernelStats,
    types::{IntentFields, PriceTicks, SharesU},
};
use blink_book::is_stale;
use blink_shadow::NoOpCode;
use blink_types::{AbortReason, RawEvent, Side, SourceKind, TimeInForce};

/// The v1 kernel. Stateless; `decide` reads only the snapshot.
#[derive(Debug, Default)]
pub struct V1Kernel;

impl V1Kernel {
    /// Construct a fresh kernel. There is no mutable state to seed —
    /// this is a convenience constructor for symmetry with future
    /// variants.
    pub fn new() -> Self {
        Self
    }
}

impl DecisionKernel for V1Kernel {
    fn impl_id(&self) -> &'static str {
        "blink-kernel-v1"
    }

    fn decide<'a>(
        &self,
        snapshot: &DecisionSnapshot<'a>,
        stats: &mut KernelStats,
    ) -> KernelVerdict<'a> {
        stats.decisions_total = stats.decisions_total.saturating_add(1);
        let verdict = decide_inner(snapshot, stats);
        match &verdict {
            KernelVerdict::Submit { .. } => stats.bump_submit(),
            KernelVerdict::Abort { reason, .. } => stats.bump_abort(*reason),
            KernelVerdict::NoOp { code } => stats.bump_noop(*code),
        }
        verdict
    }
}

// ─── Inner logic ──────────────────────────────────────────────────────────

#[inline]
fn decide_inner<'a>(snap: &DecisionSnapshot<'a>, stats: &mut KernelStats) -> KernelVerdict<'a> {
    let cfg = snap.config;
    let event = snap.event;

    // (0) Source / payload filter. Events we cannot act on (missing
    //     price / side / market id, unrecognised source) map to
    //     `NoOp{FilterMismatch}` — never a panic.
    if !is_actionable_source(event.source) {
        return KernelVerdict::NoOp { code: NoOpCode::FilterMismatch };
    }
    // (0.1) Observe-only events (e.g. mempool-tap without legal sign-off,
    //       see docs/rebuild/R3_LEGAL_MEMO_STUB.md) MUST NOT be submitted.
    //       Map to `NoOp{FilterMismatch}` — the parity fingerprint treats
    //       this as a stable code so shadow runs stay green.
    if event.observe_only {
        return KernelVerdict::NoOp { code: NoOpCode::FilterMismatch };
    }
    let (side, price_u64, size_u64, market_id) = match (
        event.side,
        event.price.map(|p| p.0),
        event.size.map(|s| s.0),
        event.market_id.as_deref(),
    ) {
        (Some(s), Some(p), Some(sz), Some(mid)) if !mid.is_empty() && !event.token_id.is_empty() => {
            (s, p, sz, mid)
        }
        _ => return KernelVerdict::NoOp { code: NoOpCode::FilterMismatch },
    };
    // Narrow to the kernel's PriceTicks(u32). Oversized ticks are
    // definitionally invalid (Polymarket ticks are ≤ 999); treat as
    // filter mismatch rather than truncating.
    let price = match u32::try_from(price_u64) {
        Ok(v) => PriceTicks(v),
        Err(_) => return KernelVerdict::NoOp { code: NoOpCode::FilterMismatch },
    };
    let size = SharesU(size_u64);
    let post_only = cfg.default_post_only;
    let tif = resolve_tif(event);

    // (1) Stale book.
    if is_stale(snap.book, snap.logical_now_ns, cfg.book_max_age_ns) {
        return KernelVerdict::Abort {
            reason: AbortReason::StaleBook,
            metric_bps: None,
        };
    }

    // (2) Drift vs reference price (book mid).
    let ref_mid = match mid_ticks(snap) {
        Some(m) => m,
        // No usable reference price ⇒ treat as stale book. Stale is the
        // most conservative disposition (the legacy pretrade_gate also
        // returns SkipStale when either side is missing).
        None => {
            return KernelVerdict::Abort {
                reason: AbortReason::StaleBook,
                metric_bps: None,
            }
        }
    };
    let drift_bps = drift_bps_i128(price.0, ref_mid);
    if drift_bps.unsigned_abs() > cfg.max_drift_bps as u128 {
        let clamped = drift_bps.clamp(i32::MIN as i128, i32::MAX as i128) as i32;
        return KernelVerdict::Abort {
            reason: AbortReason::Drift,
            metric_bps: Some(clamped),
        };
    }

    // (3) Post-only cross.
    if post_only && crosses_book(side, price.0, snap) {
        return KernelVerdict::Abort {
            reason: AbortReason::PostOnlyCross,
            metric_bps: None,
        };
    }

    // (4) Cooldown.
    if snap.position.cooldown_until_ns > snap.logical_now_ns {
        return KernelVerdict::NoOp { code: NoOpCode::CooldownActive };
    }

    // (5) Risk gate — i128 intermediate, clamped.
    match risk_check(side, price.0, size.0, snap.position, cfg) {
        RiskResult::Ok => {}
        RiskResult::LimitExceeded => {
            return KernelVerdict::Abort {
                reason: AbortReason::RiskLimit,
                metric_bps: None,
            }
        }
        RiskResult::Overflow => {
            stats.risk_denied_i128_overflow =
                stats.risk_denied_i128_overflow.saturating_add(1);
            return KernelVerdict::Abort {
                reason: AbortReason::RiskLimit,
                metric_bps: None,
            };
        }
    }

    // (6) Edge threshold (bps of limit price vs opposing book).
    let edge_bps = estimated_edge_bps(side, price.0, snap);
    if edge_bps < cfg.edge_threshold_bps as i128 {
        return KernelVerdict::NoOp { code: NoOpCode::BelowEdgeThreshold };
    }

    // (7) Build fields + semantic key, then dedup.
    let fields = IntentFields {
        token_id: event.token_id.as_str(),
        market_id,
        side,
        price,
        size,
        tif,
        post_only,
    };
    let key = fields.semantic_key();
    if snap.recent_semantic_keys.contains(&key.0) {
        return KernelVerdict::NoOp { code: NoOpCode::Dedup };
    }

    // (8) Submit.
    KernelVerdict::Submit { semantic_key: key, fields }
}

// ─── Helpers ──────────────────────────────────────────────────────────────

/// `RawEvent` is a struct, not an enum — the "non-exhaustive match" the
/// spec calls for applies to the `source` discriminant. Events that
/// don't imply an actionable trade intent map to `FilterMismatch`.
#[inline]
fn is_actionable_source(s: SourceKind) -> bool {
    match s {
        // Actionable: signals that carry a directional cue.
        SourceKind::Rn1Rest
        | SourceKind::Rn1Mempool
        | SourceKind::BullpenWs
        | SourceKind::Manual
        | SourceKind::MempoolCtf => true,
        // Passive observation sources — kernel should never submit on these.
        SourceKind::CtfLog | SourceKind::ClobWs | SourceKind::BlockchainLogs => false,
    }
}

#[inline]
fn resolve_tif(_event: &RawEvent) -> TimeInForce {
    // RawEvent does not carry tif; the v1 kernel's default is GTC.
    // Downstream risk/signer can override per-market.
    TimeInForce::Gtc
}

#[inline]
fn mid_ticks(snap: &DecisionSnapshot<'_>) -> Option<u32> {
    let bid = snap.book.bid.top()?.price_ticks;
    let ask = snap.book.ask.top()?.price_ticks;
    if bid == 0 || ask == 0 {
        return None;
    }
    // u32 + u32 cannot overflow a u64; halve after to stay tick-accurate.
    let mid = ((bid as u64) + (ask as u64)) / 2;
    u32::try_from(mid).ok()
}

/// Signed drift bps = (our_price - ref_mid) × 10_000 / ref_mid.
/// Returns 0 if `ref_mid == 0` (caller handles upstream as stale).
#[inline]
fn drift_bps_i128(our_price: u32, ref_mid: u32) -> i128 {
    if ref_mid == 0 {
        return 0;
    }
    let delta = our_price as i128 - ref_mid as i128;
    delta.saturating_mul(10_000) / ref_mid as i128
}

#[inline]
fn crosses_book(side: Side, our_price: u32, snap: &DecisionSnapshot<'_>) -> bool {
    match side {
        Side::Buy => snap
            .book
            .ask
            .top()
            .map(|a| a.price_ticks != 0 && our_price >= a.price_ticks)
            .unwrap_or(false),
        Side::Sell => snap
            .book
            .bid
            .top()
            .map(|b| b.price_ticks != 0 && our_price <= b.price_ticks)
            .unwrap_or(false),
    }
}

enum RiskResult {
    Ok,
    LimitExceeded,
    Overflow,
}

#[inline]
fn risk_check(
    side: Side,
    price: u32,
    size: u64,
    pos: &blink_book::PositionSnapshot,
    cfg: &KernelConfig,
) -> RiskResult {
    let signed_proposed: i128 = match side {
        Side::Buy => size as i128,
        Side::Sell => -(size as i128),
    };
    let new_qty = (pos.open_qty_signed_u as i128).saturating_add(signed_proposed);
    // Notional = |new_qty| * price. Bounded: |i64|*|u32| fits in i96 ⊂ i128.
    let abs_qty: i128 = new_qty.checked_abs().unwrap_or(i128::MAX);
    let notional_i128 = match abs_qty.checked_mul(price as i128) {
        Some(n) => n,
        None => return RiskResult::Overflow,
    };
    if notional_i128 < 0 {
        return RiskResult::Overflow;
    }
    // Clamp to u64 once at the end.
    let notional_u64 = if notional_i128 > u64::MAX as i128 {
        return RiskResult::LimitExceeded;
    } else {
        notional_i128 as u64
    };
    if notional_u64 > cfg.max_position_notional {
        RiskResult::LimitExceeded
    } else {
        RiskResult::Ok
    }
}

/// Edge in bps of the limit price. For a Buy at `p`, edge vs best_ask
/// `a` is `(a - p) * 10_000 / p` (positive = buying below ask). For a
/// Sell at `p` vs best_bid `b` it is `(p - b) * 10_000 / p` (positive =
/// selling above bid). Zero if either side missing.
fn estimated_edge_bps(side: Side, our_price: u32, snap: &DecisionSnapshot<'_>) -> i128 {
    if our_price == 0 {
        return 0;
    }
    let delta = match side {
        Side::Buy => match snap.book.ask.top() {
            Some(a) if a.price_ticks > 0 => a.price_ticks as i128 - our_price as i128,
            _ => return 0,
        },
        Side::Sell => match snap.book.bid.top() {
            Some(b) if b.price_ticks > 0 => our_price as i128 - b.price_ticks as i128,
            _ => return 0,
        },
    };
    delta.saturating_mul(10_000) / our_price as i128
}
