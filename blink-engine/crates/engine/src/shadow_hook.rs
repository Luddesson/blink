//! Phase 0 shadow-runner hook for paper_engine::handle_signal.
//!
//! When the `shadow` feature is enabled, this module provides a live hook that
//! observes the legacy decision and runs a parallel v1 kernel decision, capturing
//! both outcomes for offline analysis. When the feature is disabled, the hook
//! compiles to an empty inline-always stub with zero runtime cost.

/// Simplified legacy decision outcome for the hook interface.
/// 
/// This enum mirrors the `GateAbortReason` local to `paper_engine::handle_signal`
/// and keeps the hook signature free of external types when the feature is off.
#[derive(Debug, Clone, Copy)]
pub enum LegacyDecision {
    /// Gate check passed; order will be submitted.
    Submitted,
    /// Aborted due to stale book snapshot.
    AbortedStale,
    /// Aborted due to excessive drift.
    AbortedDrift { bps: i64 },
    /// Aborted due to post-only cross.
    AbortedPostOnlyCross,
    /// Reserved for future use (not currently emitted).
    NoOp,
}

#[cfg(feature = "shadow")]
mod enabled {
    use super::LegacyDecision;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::{Arc, OnceLock};
    use std::sync::atomic::{AtomicU64, Ordering};
    
    use blink_shadow::{LiveShadowRunner, CapturedRow};
    use blink_kernel::{
        KernelConfig, V1Kernel, DecisionSnapshot, KernelStats, KernelVerdict, RecentKeySet,
        verdict_to_outcome,
    };
    use blink_book::{BookSnapshot, PositionSnapshot, LadderSide, Level, BOOK_DEPTH};
    use blink_types::{RawEvent, SourceKind, Side, PriceTicks, SizeU, DecisionOutcome, AbortReason};
    use blink_timestamps::Timestamp;
    use sha3::{Digest, Keccak256};

    /// Global shadow context, initialized once at engine startup if shadow is enabled.
    pub struct ShadowCtx {
        pub runner: LiveShadowRunner,
        pub cfg: Arc<KernelConfig>,
        pub kernel: V1Kernel,
        pub panic_total: AtomicU64,
        pub event_seq: AtomicU64,
    }

    static CTX: OnceLock<Arc<ShadowCtx>> = OnceLock::new();

    /// Initialize the global shadow context. Called once from main.rs.
    pub fn init(ctx: Arc<ShadowCtx>) {
        let _ = CTX.set(ctx);
    }

    /// Access the global shadow context (None if not initialized).
    pub fn ctx() -> Option<&'static Arc<ShadowCtx>> {
        CTX.get()
    }

    /// Hot-path hook. Called from paper_engine::handle_signal after the
    /// legacy pre-trade decision is resolved, before submit.
    #[inline]
    pub fn shadow_hook(
        token_id: &str,
        market_id: &str,
        side_legacy: crate::types::OrderSide,
        price_ticks_u32: u32,
        size_u: u64,
        legacy: LegacyDecision,
        book: Option<crate::order_book::OrderBook>,
        snapshot_age_ms: Option<u32>,
    ) {
        let Some(ctx) = CTX.get() else { return };
        
        // Catch panics to prevent shadow-runner from crashing the engine
        let result = catch_unwind(AssertUnwindSafe(|| {
            run_shadow_capture(
                ctx,
                token_id,
                market_id,
                side_legacy,
                price_ticks_u32,
                size_u,
                legacy,
                book,
                snapshot_age_ms,
            )
        }));
        
        if result.is_err() {
            ctx.panic_total.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn run_shadow_capture(
        ctx: &ShadowCtx,
        token_id: &str,
        _market_id: &str,
        side_legacy: crate::types::OrderSide,
        price_ticks_u32: u32,
        size_u: u64,
        legacy: LegacyDecision,
        book: Option<crate::order_book::OrderBook>,
        _snapshot_age_ms: Option<u32>,
    ) {
        let seq = ctx.event_seq.fetch_add(1, Ordering::Relaxed);
        let logical_now_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;

        // Convert legacy decision to string
        let legacy_decision = format_decision(&legacy);

        // Convert engine-local book -> blink_book::BookSnapshot
        let book_snap = book.as_ref().map(|b| convert_book(token_id, b));
        
        // Run v1 kernel
        let (v1_decision, v1_intent_hash) = if let Some(ref book) = book_snap {
            run_v1_kernel(ctx, token_id, side_legacy, price_ticks_u32, size_u, book, logical_now_ns)
        } else {
            ("NoOp:MissingBook".to_string(), None)
        };

        // Compute legacy intent hash (simplified - just token+side+price+size)
        let legacy_intent_hash = Some(compute_intent_hash(token_id, side_legacy, price_ticks_u32, size_u));

        // Compute config hash
        let config_hash_v1 = ctx.cfg.config_hash();

        // Compute book reference hash (top bid + top ask)
        let book_ref_hash = compute_book_hash(book.as_ref());

        let row = CapturedRow {
            event_seq: seq,
            logical_now_ns,
            legacy_decision,
            v1_decision,
            legacy_intent_hash,
            v1_intent_hash,
            config_hash_v1,
            book_ref_hash,
        };

        ctx.runner.observe(row);
    }

    fn format_decision(legacy: &LegacyDecision) -> String {
        match legacy {
            LegacyDecision::Submitted => "Submitted".to_string(),
            LegacyDecision::AbortedStale => "Aborted:Stale".to_string(),
            LegacyDecision::AbortedDrift { bps } => format!("Aborted:Drift({}bps)", bps),
            LegacyDecision::AbortedPostOnlyCross => "Aborted:PostOnlyCross".to_string(),
            LegacyDecision::NoOp => "NoOp:Reserved".to_string(),
        }
    }

    fn convert_book(token_id: &str, book: &crate::order_book::OrderBook) -> BookSnapshot {
        // Extract top bid and ask
        let bid_level = book.best_bid().map(|price| Level {
            price_ticks: (price / 1000) as u32, // legacy book is ×1000, blink_book uses u32 ticks
            size_u_usdc: 0, // size not easily accessible from best_bid, leave 0 for now
        });
        
        let ask_level = book.best_ask().map(|price| Level {
            price_ticks: (price / 1000) as u32,
            size_u_usdc: 0,
        });

        let bid = if let Some(lv) = bid_level {
            LadderSide::from_slice(&[lv])
        } else {
            LadderSide::EMPTY
        };

        let ask = if let Some(lv) = ask_level {
            LadderSide::from_slice(&[lv])
        } else {
            LadderSide::EMPTY
        };

        BookSnapshot {
            token_id: token_id.to_string(),
            market_id: String::new(),
            seq: 0,
            source_wall_ns: 0,
            tsc_received: Timestamp::UNSET,
            bid,
            ask,
        }
    }

    fn run_v1_kernel(
        ctx: &ShadowCtx,
        token_id: &str,
        side: crate::types::OrderSide,
        price_ticks: u32,
        size_u: u64,
        book: &BookSnapshot,
        logical_now_ns: u64,
    ) -> (String, Option<[u8; 32]>) {
        // Build RawEvent
        let event = RawEvent {
            token_id: token_id.to_string(),
            market_id: String::new(),
            source: SourceKind::Ws,
            side: match side {
                crate::types::OrderSide::Buy => Side::Buy,
                crate::types::OrderSide::Sell => Side::Sell,
            },
            price_ticks: PriceTicks(price_ticks),
            size_u: SizeU(size_u),
            seq: 0,
            wall_ns: logical_now_ns,
        };

        // Build zero position (no state available in paper_engine context)
        let position = PositionSnapshot::zero(0);
        
        // Build decision snapshot
        let snap = DecisionSnapshot {
            event,
            book: book.clone(),
            position,
        };

        // Run kernel
        let mut stats = KernelStats::default();
        let recent_keys = RecentKeySet::new(16);
        
        let verdict_result = catch_unwind(AssertUnwindSafe(|| {
            ctx.kernel.decide(&snap, &mut stats)
        }));

        let verdict = match verdict_result {
            Ok(v) => v,
            Err(_) => {
                return ("Panic".to_string(), None);
            }
        };

        // Convert verdict to outcome
        let outcome = verdict_to_outcome(verdict, "shadow_run", 0);
        
        let decision_str = format_outcome(&outcome);
        let intent_hash = outcome.semantic_key.map(|k| k.0);

        (decision_str, intent_hash)
    }

    fn format_outcome(outcome: &DecisionOutcome) -> String {
        match outcome {
            DecisionOutcome::Submit { .. } => "Submitted".to_string(),
            DecisionOutcome::Abort { reason, .. } => {
                let reason_str = match reason {
                    AbortReason::Stale => "Stale",
                    AbortReason::Drift { bps } => return format!("Aborted:Drift({}bps)", bps),
                    AbortReason::PostOnlyCross => "PostOnlyCross",
                    AbortReason::MaxPosition => "MaxPosition",
                    AbortReason::Cooldown => "Cooldown",
                    AbortReason::InvalidBook => "InvalidBook",
                    AbortReason::EdgeThreshold => "EdgeThreshold",
                };
                format!("Aborted:{}", reason_str)
            }
            DecisionOutcome::NoOp { code, .. } => format!("NoOp:{}", code),
        }
    }

    fn compute_intent_hash(
        token_id: &str,
        side: crate::types::OrderSide,
        price_ticks: u32,
        size_u: u64,
    ) -> [u8; 32] {
        let mut hasher = Keccak256::new();
        hasher.update(token_id.as_bytes());
        hasher.update(&[match side {
            crate::types::OrderSide::Buy => 0x01,
            crate::types::OrderSide::Sell => 0x02,
        }]);
        hasher.update(&price_ticks.to_le_bytes());
        hasher.update(&size_u.to_le_bytes());
        hasher.finalize().into()
    }

    fn compute_book_hash(book: Option<&crate::order_book::OrderBook>) -> [u8; 32] {
        let mut hasher = Keccak256::new();
        if let Some(b) = book {
            if let Some(bid) = b.best_bid() {
                hasher.update(&bid.to_le_bytes());
            }
            if let Some(ask) = b.best_ask() {
                hasher.update(&ask.to_le_bytes());
            }
        }
        hasher.finalize().into()
    }
}

#[cfg(not(feature = "shadow"))]
mod disabled {
    use super::LegacyDecision;
    
    /// Zero-cost stub when shadow feature is disabled.
    #[inline(always)]
    pub fn shadow_hook(
        _token_id: &str,
        _market_id: &str,
        _side_legacy: crate::types::OrderSide,
        _price_ticks_u32: u32,
        _size_u: u64,
        _legacy: LegacyDecision,
        _book: Option<crate::order_book::OrderBook>,
        _snapshot_age_ms: Option<u32>,
    ) {
        // No-op
    }
}

#[cfg(feature = "shadow")]
pub use enabled::{shadow_hook, ShadowCtx, init, ctx};

#[cfg(not(feature = "shadow"))]
pub use disabled::shadow_hook;
