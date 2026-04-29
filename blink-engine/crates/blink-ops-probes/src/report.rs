//! Shared report helpers.

use hdrhistogram::Histogram;

/// Summarise a histogram (in microseconds) as milliseconds quantiles.
pub fn quantiles_ms(h: &Histogram<u64>) -> Quantiles {
    if h.is_empty() {
        return Quantiles::default();
    }
    Quantiles {
        count: h.len(),
        p50_ms: (h.value_at_quantile(0.50) as f64) / 1000.0,
        p90_ms: (h.value_at_quantile(0.90) as f64) / 1000.0,
        p99_ms: (h.value_at_quantile(0.99) as f64) / 1000.0,
        max_ms: (h.max() as f64) / 1000.0,
    }
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct Quantiles {
    pub count: u64,
    pub p50_ms: f64,
    pub p90_ms: f64,
    pub p99_ms: f64,
    pub max_ms: f64,
}

pub fn print_json<T: serde::Serialize>(r: &T) -> anyhow::Result<()> {
    let s = serde_json::to_string_pretty(r)?;
    println!("{s}");
    Ok(())
}
