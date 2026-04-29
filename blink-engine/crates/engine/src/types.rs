//! Core types shared across the Blink Engine crate.
//!
//! All prices and sizes are stored as [`u64`] scaled by **1 000** to avoid
//! floating-point arithmetic in hot paths.  
//! `"0.65"` → `650`, `"1500"` → `1_500_000`.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use serde::{Deserialize, Serialize};

static INTENT_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Returns the next monotonic intent ID for signal tracking.
/// Called at signal ingress from each producer (sniffer, rn1_poller, etc.).
#[inline]
pub fn next_intent_id() -> u64 {
    INTENT_ID_COUNTER.fetch_add(1, Ordering::Relaxed)
}

// ─── Order Side ─────────────────────────────────────────────────────────────

/// Direction of an order or trade.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum OrderSide {
    Buy,
    Sell,
}

impl std::fmt::Display for OrderSide {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OrderSide::Buy => write!(f, "BUY"),
            OrderSide::Sell => write!(f, "SELL"),
        }
    }
}

// ─── Time-in-Force ──────────────────────────────────────────────────────────

/// Time-in-force for CLOB orders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TimeInForce {
    /// Good Till Cancelled — standard maker order.
    Gtc,
    /// Fill Or Kill — fill fully at exact price or cancel immediately.
    Fok,
    /// Fill And Kill — fill as much as possible then cancel remainder.
    Fak,
}

impl std::fmt::Display for TimeInForce {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TimeInForce::Gtc => write!(f, "GTC"),
            TimeInForce::Fok => write!(f, "FOK"),
            TimeInForce::Fak => write!(f, "FAK"),
        }
    }
}

impl Default for TimeInForce {
    fn default() -> Self {
        TimeInForce::Gtc
    }
}

// ─── Price Levels ────────────────────────────────────────────────────────────

/// An order-book price level with both fields scaled by 1 000.
///
/// Use [`parse_price`] to construct from raw API strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PriceLevel {
    /// Price × 1 000 (e.g. `0.65` → `650`).
    pub price: u64,
    /// Size × 1 000 (e.g. `1500` → `1_500_000`).
    pub size: u64,
}

/// Raw price-level as received from the WebSocket (string-encoded fields).
///
/// Call [`RawPriceLevel::to_price_level`] to get a scaled [`PriceLevel`].
#[derive(Debug, Clone, Deserialize)]
pub struct RawPriceLevel {
    pub price: String,
    pub size: String,
}

impl RawPriceLevel {
    /// Converts string fields to a [`PriceLevel`] scaled by 1 000.
    #[inline]
    pub fn to_price_level(&self) -> PriceLevel {
        PriceLevel {
            price: parse_price(&self.price),
            size: parse_price(&self.size),
        }
    }
}

// ─── WebSocket Event Payloads ─────────────────────────────────────────────────

/// Payload carried by `"book"` WebSocket events — full order-book snapshot.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct BookEvent {
    /// Polymarket condition ID (0x…).
    pub market: String,
    /// Polymarket token/asset ID (the long numeric string).
    pub asset_id: Option<String>,
    /// Bid levels — size "0" means the level should be removed.
    #[serde(default)]
    pub bids: Vec<RawPriceLevel>,
    /// Ask levels — size "0" means the level should be removed.
    #[serde(default)]
    pub asks: Vec<RawPriceLevel>,
    pub timestamp: Option<String>,
    pub hash: Option<String>,
}

/// A single entry inside a `"price_change"` event's `price_changes` array.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct PriceChangeEntry {
    /// Token/asset ID this change applies to.
    pub asset_id: String,
    pub price: String,
    pub size: String,
    pub side: OrderSide,
    #[serde(default)]
    pub hash: Option<String>,
    #[serde(default)]
    pub best_bid: Option<String>,
    #[serde(default)]
    pub best_ask: Option<String>,
}

/// Payload carried by `"price_change"` WebSocket events — incremental deltas.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct PriceChangeEvent {
    /// Polymarket condition ID (0x…).
    pub market: String,
    /// Individual price-level changes, each tagged with asset_id and side.
    pub price_changes: Vec<PriceChangeEntry>,
    pub timestamp: Option<String>,
}

/// Payload carried by `"last_trade_price"` WebSocket events.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct TradeEvent {
    pub market: String,
    #[serde(default)]
    pub asset_id: Option<String>,
    pub price: String,
    pub size: String,
    pub side: OrderSide,
    pub timestamp: Option<String>,
    #[serde(default)]
    pub fee_rate_bps: Option<String>,
}

/// Payload carried by `"order"` WebSocket events.
///
/// This is the primary event inspected by the [`crate::sniffer::Sniffer`].
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct OrderEvent {
    pub market: String,
    #[serde(default)]
    pub asset_id: Option<String>,
    pub order_id: String,
    /// On-chain wallet address of the order placer.
    pub owner: String,
    pub side: OrderSide,
    pub price: String,
    pub size_matched: Option<String>,
    pub original_size: String,
    /// `"LIMIT"` or `"MARKET"`. Renamed from the reserved word `type`.
    #[serde(rename = "type")]
    pub order_type: String,
    pub created_at: Option<String>,
}

// ─── Market Event (top-level discriminated union) ────────────────────────────

/// Parsed WebSocket market event, discriminated on the `event_type` field.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "event_type")]
pub enum MarketEvent {
    /// Full order-book snapshot (sent on subscribe or reconnect).
    #[serde(rename = "book")]
    Book(BookEvent),

    /// Incremental order-book delta (new order placed or cancelled).
    #[serde(rename = "price_change")]
    PriceChange(PriceChangeEvent),

    /// Last executed trade for a market.
    #[allow(dead_code)]
    #[serde(rename = "last_trade_price")]
    LastTradePrice(TradeEvent),

    /// A new CLOB order placed — the primary RN1 detection target.
    #[serde(rename = "order")]
    Order(OrderEvent),

    /// Any event type we don't handle (tick_size_change, best_bid_ask,
    /// new_market, market_resolved, etc.). Silently ignored.
    #[serde(other)]
    Unknown,
}

// ─── RN1 Signal ──────────────────────────────────────────────────────────────

/// Signal emitted by the sniffer when an order from the tracked RN1 wallet
/// is detected on the WebSocket feed.
#[derive(Debug, Clone)]
pub struct RN1Signal {
    /// Polymarket token ID the order was placed on.
    pub token_id: String,
    /// Optional human-readable market title.
    pub market_title: Option<String>,
    /// Optional human-readable selected outcome/side label.
    pub market_outcome: Option<String>,
    pub side: OrderSide,
    /// Limit price × 1 000.
    pub price: u64,
    /// `original_size` × 1 000.
    pub size: u64,
    pub order_id: String,
    /// Wall-clock timestamp recorded at the moment of detection.
    pub detected_at: Instant,
    /// Unix timestamp — game/event kickoff time (from Gamma API).
    pub event_start_time: Option<i64>,
    /// Unix timestamp — market resolution deadline (from Gamma API).
    pub event_end_time: Option<i64>,
    /// Address of the tracked wallet that originated this signal.
    pub source_wallet: String,
    /// Sizing weight for this wallet (1.0 = primary, <1.0 = secondary).
    pub wallet_weight: f64,
    /// Signal source: "rn1" or "alpha".
    pub signal_source: String,
    /// Analysis ID from the alpha sidecar (for position→signal correlation).
    pub analysis_id: Option<String>,
    /// Monotonic ID generated at ingress; used for cross-system signal correlation.
    pub intent_id: u64,
    /// Polymarket condition_id / market id, if available from the signal source.
    pub market_id: Option<String>,
    /// Upstream wallet/trade id from the source system (tx hash, order id, etc.).
    pub source_order_id: Option<String>,
    /// Sequence number from the upstream source feed, if present.
    pub source_seq: Option<u64>,
    /// Wall-clock instant at which the signal was placed into the inbound channel.
    /// Not serialized — stamped at the production site immediately before `try_send`.
    pub enqueued_at: std::time::Instant,
}

/// Simplified signal used for position tracking and hedge detection
#[derive(Debug, Clone)]
pub struct Signal {
    /// Market identifier (condition_id or other unique market ID)
    pub market_id: String,
    /// Token ID within the market
    pub token_id: String,
    pub side: OrderSide,
    /// Size in USDC (not scaled)
    pub size: f64,
    /// Price as decimal (e.g. 0.55, not scaled)
    pub price: f64,
}

// ─── Filter Config ───────────────────────────────────────────────────────────

/// Configuration for filtering RN1 signals based on deep analysis
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterConfig {
    // Bet size filters
    pub min_rn1_bet_usdc: f64,
    pub max_rn1_bet_usdc: f64,

    // Market filters
    pub min_market_liquidity_usdc: f64,
    pub preferred_categories: Vec<String>,
    pub allowed_sports: Vec<String>,

    // Entry price filters
    pub min_entry_price: f64,
    pub max_entry_price: f64,

    // Timing filters (in seconds)
    pub min_seconds_before_event: i64,
    pub max_seconds_before_event: i64,

    // Dynamic sizing parameters
    pub base_multiplier: f64,
    pub whale_bet_threshold_usdc: f64,
    pub whale_bonus_multiplier: f64,
    pub high_liquidity_threshold_usdc: f64,
    pub high_liquidity_bonus: f64,
    pub sports_bonus: f64,
    pub preferred_sport_bonus: f64,
    pub max_multiplier: f64,

    // Position limits
    pub max_position_pct: f64,
    pub max_concurrent_positions: usize,
}

impl Default for FilterConfig {
    fn default() -> Self {
        Self {
            // RN1 bet size: $10k-$100k sweet spot
            min_rn1_bet_usdc: 10_000.0,
            max_rn1_bet_usdc: 100_000.0,

            // Market liquidity: $100k+ only
            min_market_liquidity_usdc: 100_000.0,

            // Categories: Sports primary, high-liquidity politics secondary
            preferred_categories: vec!["sports".to_string(), "politics".to_string()],

            // Sports focus: Soccer (37%), NFL (26%), NBA (11%)
            allowed_sports: vec![
                "Soccer".to_string(),
                "Football".to_string(),
                "NFL".to_string(),
                "NBA".to_string(),
                "Basketball".to_string(),
                "MLB".to_string(),
                "Baseball".to_string(),
            ],

            // Entry price: 25-65¢ range (RN1's sweet spot ~34¢)
            min_entry_price: 0.25,
            max_entry_price: 0.65,

            // Timing: 2-72 hours before event
            min_seconds_before_event: 2 * 3600,
            max_seconds_before_event: 72 * 3600,

            // Dynamic sizing: 5% base, up to 15% for high conviction
            base_multiplier: 0.05,
            whale_bet_threshold_usdc: 50_000.0,
            whale_bonus_multiplier: 0.05,
            high_liquidity_threshold_usdc: 200_000.0,
            high_liquidity_bonus: 0.02,
            sports_bonus: 0.02,
            preferred_sport_bonus: 0.01,
            max_multiplier: 0.15,

            // Risk limits
            max_position_pct: 0.15,
            max_concurrent_positions: 5,
        }
    }
}

impl FilterConfig {
    /// Load from .env file or use defaults.
    ///
    /// Example usage:
    /// ```
    /// use engine::types::FilterConfig;
    /// let config = FilterConfig::from_env();
    /// // Override with environment variables:
    /// // MIN_RN1_BET_USDC=15000.0
    /// // MAX_RN1_BET_USDC=75000.0
    /// ```
    pub fn from_env() -> Self {
        let mut config = Self::default();

        if let Ok(val) = std::env::var("MIN_RN1_BET_USDC") {
            config.min_rn1_bet_usdc = val.parse().unwrap_or(config.min_rn1_bet_usdc);
        }
        if let Ok(val) = std::env::var("MAX_RN1_BET_USDC") {
            config.max_rn1_bet_usdc = val.parse().unwrap_or(config.max_rn1_bet_usdc);
        }
        if let Ok(val) = std::env::var("MIN_MARKET_LIQUIDITY_USDC") {
            config.min_market_liquidity_usdc =
                val.parse().unwrap_or(config.min_market_liquidity_usdc);
        }
        if let Ok(val) = std::env::var("MIN_ENTRY_PRICE") {
            config.min_entry_price = val.parse().unwrap_or(config.min_entry_price);
        }
        if let Ok(val) = std::env::var("MAX_ENTRY_PRICE") {
            config.max_entry_price = val.parse().unwrap_or(config.max_entry_price);
        }
        if let Ok(val) = std::env::var("BASE_MULTIPLIER") {
            config.base_multiplier = val.parse().unwrap_or(config.base_multiplier);
        }
        if let Ok(val) = std::env::var("MAX_MULTIPLIER") {
            config.max_multiplier = val.parse().unwrap_or(config.max_multiplier);
        }
        if let Ok(val) = std::env::var("MAX_POSITION_PCT") {
            config.max_position_pct = val.parse().unwrap_or(config.max_position_pct);
        }
        if let Ok(val) = std::env::var("MAX_CONCURRENT_POSITIONS") {
            config.max_concurrent_positions =
                val.parse().unwrap_or(config.max_concurrent_positions);
        }

        config
    }
}

// ─── Skip Reason ──────────────────────────────────────────────────────────────

/// Reasons why a signal was filtered/skipped
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SkipReason {
    BetSizeTooSmall(f64),
    BetSizeTooLarge(f64),
    LiquidityTooLow(f64),
    CategoryNotPreferred(String),
    SportNotAllowed(String),
    EntryPriceTooLow(f64),
    EntryPriceTooHigh(f64),
    EventTooSoon(i64),
    EventTooFar(i64),
    HedgeDetected,
    RiskLimitReached(String),
    MetadataFetchFailed,
    None,
}

impl fmt::Display for SkipReason {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::BetSizeTooSmall(size) => write!(f, "Bet size ${:.0} below minimum", size),
            Self::BetSizeTooLarge(size) => write!(f, "Bet size ${:.0} above maximum", size),
            Self::LiquidityTooLow(liq) => write!(f, "Market liquidity ${:.0} too low", liq),
            Self::CategoryNotPreferred(cat) => write!(f, "Category '{}' not preferred", cat),
            Self::SportNotAllowed(sport) => write!(f, "Sport '{}' not in allowed list", sport),
            Self::EntryPriceTooLow(price) => write!(f, "Entry price {:.3} too low", price),
            Self::EntryPriceTooHigh(price) => write!(f, "Entry price {:.3} too high", price),
            Self::EventTooSoon(secs) => write!(f, "Event in {} hours (too soon)", secs / 3600),
            Self::EventTooFar(secs) => write!(f, "Event in {} hours (too far)", secs / 3600),
            Self::HedgeDetected => write!(f, "Hedge trade detected (synthetic close)"),
            Self::RiskLimitReached(msg) => write!(f, "Risk limit: {}", msg),
            Self::MetadataFetchFailed => write!(f, "Failed to fetch market metadata"),
            Self::None => write!(f, "No skip reason"),
        }
    }
}

// Example usage:
// let config = FilterConfig::default();
// if signal.size < config.min_rn1_bet_usdc {
//     return SkipReason::BetSizeTooSmall(signal.size);
// }

// ─── Market Metadata ──────────────────────────────────────────────────────────

/// Market metadata for filtering decisions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketMetadata {
    pub market_id: String,
    pub token_id: String,
    pub category: String,
    pub tags: Vec<String>,
    pub volume_24h: f64,
    pub liquidity: f64,
    pub event_start_time: Option<i64>, // Unix timestamp — game/event kickoff
    pub event_end_time: Option<i64>,   // Unix timestamp — market resolution deadline
    pub closed: bool,
    /// True when the market requires Polymarket's neg-risk order path.
    pub neg_risk: bool,
    /// Gamma's augmented/enable flag, when present.
    pub enable_neg_risk: bool,
    /// Minimum allowed price increment as reported by Gamma/CLOB metadata.
    pub minimum_tick_size: Option<String>,
}

impl MarketMetadata {
    /// Check if this market matches FilterConfig criteria
    pub fn is_viable(&self, config: &FilterConfig) -> Result<(), SkipReason> {
        // Liquidity check
        if self.liquidity < config.min_market_liquidity_usdc {
            return Err(SkipReason::LiquidityTooLow(self.liquidity));
        }

        // Category check
        if !config
            .preferred_categories
            .iter()
            .any(|c| self.category.contains(c))
        {
            return Err(SkipReason::CategoryNotPreferred(self.category.clone()));
        }

        // Timing check (if event_start_time available)
        if let Some(event_time) = self.event_start_time {
            let now = chrono::Utc::now().timestamp();
            let seconds_until = event_time - now;

            if seconds_until < config.min_seconds_before_event {
                return Err(SkipReason::EventTooSoon(seconds_until));
            }
            if seconds_until > config.max_seconds_before_event {
                return Err(SkipReason::EventTooFar(seconds_until));
            }
        }

        // Market closed check
        if self.closed {
            return Err(SkipReason::RiskLimitReached("Market is closed".to_string()));
        }

        Ok(())
    }

    /// Extract sports category if present
    pub fn extract_sport(&self) -> Option<String> {
        for tag in &self.tags {
            let tag_lower = tag.to_lowercase();
            if tag_lower.contains("soccer") || tag_lower.contains("football") {
                return Some("Soccer".to_string());
            }
            if tag_lower.contains("nfl") {
                return Some("NFL".to_string());
            }
            if tag_lower.contains("nba") || tag_lower.contains("basketball") {
                return Some("NBA".to_string());
            }
            if tag_lower.contains("mlb") || tag_lower.contains("baseball") {
                return Some("MLB".to_string());
            }
        }
        None
    }
}

// ─── Price Parser ─────────────────────────────────────────────────────────────

/// Converts a decimal-string price to a [`u64`] scaled by **1 000**.
///
/// Up to three fractional digits are preserved; additional digits are
/// truncated (not rounded).
///
/// # Examples
/// ```
/// use engine::types::parse_price;
/// assert_eq!(parse_price("0.65"),    650);
/// assert_eq!(parse_price("1.00"),  1_000);
/// assert_eq!(parse_price("0.1"),     100);
/// assert_eq!(parse_price("50000"), 50_000_000);
/// assert_eq!(parse_price("0"),         0);
/// ```
pub fn parse_price(s: &str) -> u64 {
    let s = s.trim();
    match s.find('.') {
        Some(dot_pos) => {
            let int_part: u64 = s[..dot_pos].parse().unwrap_or(0);
            let frac_str = &s[dot_pos + 1..];
            // Keep at most 3 fractional digits, zero-pad on the right.
            let frac_chars: String = frac_str.chars().take(3).collect();
            let frac_padded = format!("{:0<3}", frac_chars);
            let frac: u64 = frac_padded.parse().unwrap_or(0);
            int_part * 1_000 + frac
        }
        None => s.parse::<u64>().unwrap_or(0) * 1_000,
    }
}

/// Formats an internal u64 price (×1 000) back to a human-readable decimal.
///
/// `650` → `"0.650"`.
pub fn format_price(p: u64) -> String {
    format!("{}.{:03}", p / 1_000, p % 1_000)
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_price_examples() {
        assert_eq!(parse_price("0.65"), 650);
        assert_eq!(parse_price("1.00"), 1_000);
        assert_eq!(parse_price("0.1"), 100);
        assert_eq!(parse_price("0.001"), 1);
        assert_eq!(parse_price("0.0014"), 1); // truncated, not rounded
        assert_eq!(parse_price("50000"), 50_000_000);
        assert_eq!(parse_price("0"), 0);
        assert_eq!(parse_price("  0.99  "), 990); // whitespace stripped
    }

    #[test]
    fn format_price_roundtrip() {
        assert_eq!(format_price(650), "0.650");
        assert_eq!(format_price(1_000), "1.000");
        assert_eq!(format_price(990), "0.990");
    }
}
