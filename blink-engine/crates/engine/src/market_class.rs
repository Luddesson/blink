//! Fine-grained market classification.
//!
//! [`MarketMetadata::category`] from the Gamma API is coarse
//! (typically just `"sports"`, `"politics"`, or `"crypto"`), which is too
//! blunt for tuning risk knobs like the drift gate. This module inspects
//! `category` **and** `tags` to derive a more useful [`MarketClass`].
//!
//! Classification is pure (no I/O) and case-insensitive. Used by
//! [`crate::drift_overrides`] to apply category-specific drift thresholds.

use crate::types::MarketMetadata;

/// Fine-grained market class used for category-aware risk overrides.
///
/// Variants are matched case-insensitively against tag/category substrings.
/// The order below is also the matching priority inside
/// [`MarketClass::from_tags_and_category`] (sports ranks before the generic
/// `Sports` bucket, esports before the generic, etc.).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MarketClass {
    // High-vol / fast-moving sports (justify wider drift tolerance).
    Tennis,
    Ufc,
    Boxing,
    // Major team sports.
    Soccer,
    Nfl,
    Nba,
    Mlb,
    Nhl,
    // Esports.
    Cs2,
    Valorant,
    Dota,
    Lol,
    // Generic buckets.
    Sports,
    Crypto,
    Geopolitics,
    Politics,
    Other,
}

impl MarketClass {
    /// All known classes in a canonical order (used for diagnostics and tests).
    pub const ALL: &'static [MarketClass] = &[
        MarketClass::Tennis,
        MarketClass::Ufc,
        MarketClass::Boxing,
        MarketClass::Soccer,
        MarketClass::Nfl,
        MarketClass::Nba,
        MarketClass::Mlb,
        MarketClass::Nhl,
        MarketClass::Cs2,
        MarketClass::Valorant,
        MarketClass::Dota,
        MarketClass::Lol,
        MarketClass::Sports,
        MarketClass::Crypto,
        MarketClass::Geopolitics,
        MarketClass::Politics,
        MarketClass::Other,
    ];

    /// Canonical lowercase identifier used in env config
    /// (e.g. `BLINK_GATE_DRIFT_BPS_OVERRIDES=tennis=50,cs2=200`).
    pub fn as_str(self) -> &'static str {
        match self {
            MarketClass::Tennis => "tennis",
            MarketClass::Ufc => "ufc",
            MarketClass::Boxing => "boxing",
            MarketClass::Soccer => "soccer",
            MarketClass::Nfl => "nfl",
            MarketClass::Nba => "nba",
            MarketClass::Mlb => "mlb",
            MarketClass::Nhl => "nhl",
            MarketClass::Cs2 => "cs2",
            MarketClass::Valorant => "valorant",
            MarketClass::Dota => "dota",
            MarketClass::Lol => "lol",
            MarketClass::Sports => "sports",
            MarketClass::Crypto => "crypto",
            MarketClass::Geopolitics => "geopolitics",
            MarketClass::Politics => "politics",
            MarketClass::Other => "other",
        }
    }

    /// Parse a lowercase, trimmed canonical identifier. Returns `None` for
    /// unknown strings so callers can error out on misconfig (see
    /// [`crate::drift_overrides`]).
    pub fn from_canonical_str(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|c| c.as_str() == s)
    }

    /// Derive a class from a free-text market title (e.g. the `market_title`
    /// on an [`crate::types::RN1Signal`]). Uses the same keyword set as
    /// [`Self::from_tags_and_category`]; returns [`MarketClass::Other`]
    /// when the title is empty or doesn't match any known keyword.
    pub fn from_title(title: &str) -> Self {
        if title.is_empty() {
            return MarketClass::Other;
        }
        // Reuse the tags+category matcher by treating the title as a single tag.
        Self::from_tags_and_category(&[title.to_string()], "")
    }

    /// Convenience for an `Option<&str>` title (common on signals).
    pub fn from_title_opt(title: Option<&str>) -> Self {
        title.map(Self::from_title).unwrap_or(MarketClass::Other)
    }

    /// Derive a class from raw tag strings and a category string.
    ///
    /// Matching is substring, case-insensitive. Highest-specificity match wins
    /// (e.g. `tags=["Tennis", "Madrid Open"]` → `Tennis`, not `Sports`).
    pub fn from_tags_and_category(tags: &[String], category: &str) -> Self {
        let cat_lc = category.to_lowercase();
        let tags_lc: Vec<String> = tags.iter().map(|t| t.to_lowercase()).collect();
        let any = |needles: &[&str]| -> bool {
            tags_lc.iter().any(|t| needles.iter().any(|n| t.contains(n)))
                || needles.iter().any(|n| cat_lc.contains(n))
        };

        // Order matters: most specific first.
        if any(&["tennis", "atp", "wta"]) {
            return MarketClass::Tennis;
        }
        if any(&["ufc", "mma"]) {
            return MarketClass::Ufc;
        }
        if any(&["boxing"]) {
            return MarketClass::Boxing;
        }
        if any(&["cs2", "counter-strike", "cs:go", "csgo"]) {
            return MarketClass::Cs2;
        }
        if any(&["valorant"]) {
            return MarketClass::Valorant;
        }
        if any(&["dota"]) {
            return MarketClass::Dota;
        }
        if any(&["league of legends", " lol "]) || tags_lc.iter().any(|t| t == "lol") {
            return MarketClass::Lol;
        }
        if any(&["nfl"]) {
            return MarketClass::Nfl;
        }
        if any(&["nba", "basketball"]) {
            return MarketClass::Nba;
        }
        if any(&["mlb", "baseball"]) {
            return MarketClass::Mlb;
        }
        if any(&["nhl", "hockey"]) {
            return MarketClass::Nhl;
        }
        if any(&["soccer", "football", "premier league", "la liga", "champions league"]) {
            // Polymarket labels American football as "NFL"; generic "football" here
            // means association football / soccer.
            return MarketClass::Soccer;
        }
        if any(&["geopolit", "sanction", "nato", "treaty", "military", "war"]) {
            return MarketClass::Geopolitics;
        }
        if any(&["election", "politic", "senate", "congress", "president"]) {
            return MarketClass::Politics;
        }
        if any(&["crypto", "bitcoin", "ethereum", "btc", "eth"]) {
            return MarketClass::Crypto;
        }
        if any(&["sport"]) {
            return MarketClass::Sports;
        }
        MarketClass::Other
    }
}

/// Convenience classifier for a full [`MarketMetadata`].
pub fn classify(meta: &MarketMetadata) -> MarketClass {
    MarketClass::from_tags_and_category(&meta.tags, &meta.category)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(category: &str, tags: &[&str]) -> MarketMetadata {
        MarketMetadata {
            market_id: "m".into(),
            token_id: "t".into(),
            category: category.into(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            volume_24h: 0.0,
            liquidity: 0.0,
            event_start_time: None,
            event_end_time: None,
            closed: false,
        }
    }

    #[test]
    fn tennis_beats_sports() {
        assert_eq!(classify(&meta("sports", &["Tennis", "Madrid Open"])), MarketClass::Tennis);
        assert_eq!(classify(&meta("sports", &["ATP", "Madrid"])), MarketClass::Tennis);
        assert_eq!(classify(&meta("sports", &["WTA"])), MarketClass::Tennis);
    }

    #[test]
    fn cs2_variants() {
        for v in ["CS2", "Counter-Strike 2", "CSGO", "CS:GO"] {
            assert_eq!(classify(&meta("esports", &[v])), MarketClass::Cs2, "{v}");
        }
    }

    #[test]
    fn soccer_variants() {
        assert_eq!(classify(&meta("sports", &["Soccer"])), MarketClass::Soccer);
        assert_eq!(classify(&meta("sports", &["Premier League"])), MarketClass::Soccer);
        assert_eq!(classify(&meta("sports", &["Champions League"])), MarketClass::Soccer);
        // Generic "football" → soccer (Polymarket labels NFL explicitly).
        assert_eq!(classify(&meta("sports", &["Football"])), MarketClass::Soccer);
    }

    #[test]
    fn nfl_stays_nfl() {
        assert_eq!(classify(&meta("sports", &["NFL", "Football"])), MarketClass::Nfl);
    }

    #[test]
    fn generic_sports_fallback() {
        assert_eq!(classify(&meta("sports", &["Unknown Tag"])), MarketClass::Sports);
    }

    #[test]
    fn politics_and_geopolitics() {
        assert_eq!(classify(&meta("politics", &["US Election"])), MarketClass::Politics);
        assert_eq!(classify(&meta("news", &["NATO", "Sanctions"])), MarketClass::Geopolitics);
    }

    #[test]
    fn crypto() {
        assert_eq!(classify(&meta("crypto", &["Bitcoin"])), MarketClass::Crypto);
        assert_eq!(classify(&meta("crypto", &["ETH price"])), MarketClass::Crypto);
    }

    #[test]
    fn unknown_is_other() {
        assert_eq!(classify(&meta("weather", &["Hurricane"])), MarketClass::Other);
    }

    #[test]
    fn from_canonical_str_roundtrip() {
        for c in MarketClass::ALL {
            assert_eq!(MarketClass::from_canonical_str(c.as_str()), Some(*c));
        }
        assert_eq!(MarketClass::from_canonical_str("not-a-class"), None);
        assert_eq!(MarketClass::from_canonical_str(""), None);
    }

    #[test]
    fn from_title_matches_tennis() {
        assert_eq!(MarketClass::from_title("ATP Madrid Open: Snigur vs Vallejo"), MarketClass::Tennis);
        assert_eq!(MarketClass::from_title("WTA final"), MarketClass::Tennis);
    }

    #[test]
    fn from_title_empty_or_missing_is_other() {
        assert_eq!(MarketClass::from_title(""), MarketClass::Other);
        assert_eq!(MarketClass::from_title_opt(None), MarketClass::Other);
        assert_eq!(MarketClass::from_title_opt(Some("")), MarketClass::Other);
        assert_eq!(MarketClass::from_title("Will it rain Tuesday?"), MarketClass::Other);
    }

    #[test]
    fn from_title_matches_cs2() {
        assert_eq!(MarketClass::from_title("CS2 Major: NAVI vs FaZe"), MarketClass::Cs2);
        assert_eq!(MarketClass::from_title("Counter-Strike IEM Katowice"), MarketClass::Cs2);
    }

    #[test]
    fn case_insensitive() {
        assert_eq!(classify(&meta("SPORTS", &["TENNIS"])), MarketClass::Tennis);
        assert_eq!(classify(&meta("Sports", &["Cs2"])), MarketClass::Cs2);
    }
}
