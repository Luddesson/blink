//! Lightweight live/shadow signal scoring for quant canary decisions.
//!
//! This module is deliberately pure: it has no network, database, or engine
//! side effects, which makes it safe to use both in live audit paths and tests.

#[derive(Debug, Clone, Copy)]
pub struct QuantSignalFeatures {
    pub price_u64: u64,
    pub rn1_notional_usd: f64,
    pub intended_size_usdc: Option<f64>,
    pub spread_bps: Option<u64>,
    pub book_age_ms: Option<u64>,
    pub contra_depth_usdc: Option<f64>,
    pub market_liquidity_usd: Option<f64>,
    pub volume_24h_usd: Option<f64>,
    pub neg_risk: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuantSignalScore {
    pub score_bps: i64,
    pub grade: &'static str,
    pub toxicity_bps: i64,
    pub shadow_decision: &'static str,
    pub shadow_reason: &'static str,
}

impl QuantSignalScore {
    pub fn passes(&self, min_score_bps: i64, max_toxicity_bps: i64) -> bool {
        self.score_bps >= min_score_bps && self.toxicity_bps <= max_toxicity_bps
    }
}

pub fn score_signal(features: QuantSignalFeatures) -> QuantSignalScore {
    let rn1_notional = features.rn1_notional_usd.max(0.0);
    let intended_size = features.intended_size_usdc.unwrap_or(0.0).max(0.0);
    let depth = features.contra_depth_usdc.unwrap_or(0.0).max(0.0);

    let notional_score = linear_score(rn1_notional, 5.0, 250.0, 2_000.0);
    let depth_score = if intended_size > 0.0 {
        linear_score(depth / intended_size, 0.25, 2.5, 2_500.0)
    } else if depth > 0.0 {
        750
    } else {
        0
    };
    let spread_score = match features.spread_bps {
        Some(spread) if spread <= 50 => 2_000,
        Some(spread) if spread <= 750 => {
            let ratio = (750 - spread) as f64 / 700.0;
            (ratio * 2_000.0).round() as i64
        }
        Some(_) => 0,
        None => 0,
    };
    let recency_score = match features.book_age_ms {
        Some(age) if age <= 500 => 1_500,
        Some(age) if age <= 5_000 => {
            let ratio = (5_000 - age) as f64 / 4_500.0;
            (ratio * 1_500.0).round() as i64
        }
        Some(_) => 0,
        None => 0,
    };
    let price_score = price_quality_score(features.price_u64);
    let liquidity_score = features
        .market_liquidity_usd
        .map(|v| linear_score(v.max(0.0), 5_000.0, 250_000.0, 700.0))
        .unwrap_or(0);
    let volume_score = features
        .volume_24h_usd
        .map(|v| linear_score(v.max(0.0), 5_000.0, 500_000.0, 300.0))
        .unwrap_or(0);

    let toxicity_bps = toxicity_bps(features, intended_size, depth);
    let raw_score = notional_score
        + depth_score
        + spread_score
        + recency_score
        + price_score
        + liquidity_score
        + volume_score
        - toxicity_bps / 4;
    let score_bps = raw_score.clamp(0, 10_000);
    let grade = grade_for_score(score_bps);
    let (shadow_decision, shadow_reason) = shadow_decision_for(score_bps, toxicity_bps);

    QuantSignalScore {
        score_bps,
        grade,
        toxicity_bps,
        shadow_decision,
        shadow_reason,
    }
}

fn linear_score(value: f64, min: f64, max: f64, points: f64) -> i64 {
    if !value.is_finite() || value <= min {
        return 0;
    }
    if value >= max {
        return points.round() as i64;
    }
    (((value - min) / (max - min)) * points).round() as i64
}

fn price_quality_score(price_u64: u64) -> i64 {
    match price_u64 {
        100..=900 => 1_000,
        20..=980 => 650,
        1..=999 => 250,
        _ => 0,
    }
}

fn toxicity_bps(features: QuantSignalFeatures, intended_size: f64, depth: f64) -> i64 {
    let mut toxicity = 0i64;

    match features.spread_bps {
        Some(spread) if spread <= 100 => {}
        Some(spread) => toxicity += ((spread.saturating_sub(100) as i64) * 3).min(4_000),
        None => toxicity += 2_000,
    }

    match features.book_age_ms {
        Some(age) if age <= 1_000 => {}
        Some(age) => toxicity += ((age.saturating_sub(1_000) as i64) / 2).min(3_000),
        None => toxicity += 2_000,
    }

    if intended_size > 0.0 {
        let coverage = depth / intended_size;
        if coverage < 1.0 {
            toxicity += ((1.0 - coverage.max(0.0)) * 2_500.0).round() as i64;
        }
    } else if depth <= 0.0 {
        toxicity += 1_000;
    }

    if !(1..=999).contains(&features.price_u64) {
        toxicity += 2_000;
    } else if !(20..=980).contains(&features.price_u64) {
        toxicity += 750;
    }

    if features.neg_risk {
        toxicity += 3_000;
    }

    toxicity.clamp(0, 20_000)
}

fn grade_for_score(score_bps: i64) -> &'static str {
    match score_bps {
        8_000..=10_000 => "A",
        6_500..=7_999 => "B",
        5_000..=6_499 => "C",
        3_500..=4_999 => "D",
        _ => "F",
    }
}

fn shadow_decision_for(score_bps: i64, toxicity_bps: i64) -> (&'static str, &'static str) {
    if toxicity_bps > 12_000 {
        ("quant_block_shadow", "toxicity_above_shadow_limit")
    } else if score_bps < 4_500 {
        ("quant_block_shadow", "score_below_shadow_floor")
    } else {
        ("quant_accept_shadow", "score_passed_shadow_floor")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_features() -> QuantSignalFeatures {
        QuantSignalFeatures {
            price_u64: 520,
            rn1_notional_usd: 125.0,
            intended_size_usdc: Some(2.0),
            spread_bps: Some(60),
            book_age_ms: Some(250),
            contra_depth_usdc: Some(15.0),
            market_liquidity_usd: Some(100_000.0),
            volume_24h_usd: Some(150_000.0),
            neg_risk: false,
        }
    }

    #[test]
    fn score_signal_rewards_fresh_tight_deep_books() {
        let score = score_signal(base_features());

        assert!(score.score_bps >= 6_500, "score={score:?}");
        assert_eq!(score.shadow_decision, "quant_accept_shadow");
    }

    #[test]
    fn score_signal_penalizes_stale_wide_shallow_books() {
        let score = score_signal(QuantSignalFeatures {
            spread_bps: Some(2_500),
            book_age_ms: Some(20_000),
            contra_depth_usdc: Some(0.10),
            ..base_features()
        });

        assert!(score.score_bps < 4_500, "score={score:?}");
        assert_eq!(score.shadow_decision, "quant_block_shadow");
    }

    #[test]
    fn score_signal_penalizes_neg_risk() {
        let clean = score_signal(base_features());
        let neg_risk = score_signal(QuantSignalFeatures {
            neg_risk: true,
            ..base_features()
        });

        assert!(neg_risk.score_bps < clean.score_bps);
        assert!(neg_risk.toxicity_bps > clean.toxicity_bps);
    }
}
