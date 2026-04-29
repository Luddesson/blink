//! R-2: Polymarket CLOB rate-limit probe.
//!
//! Sends POST /order at a target RPS with a deliberately-rejected payload
//! (bad signature — we measure the rate-limit layer, not execution) and
//! records HTTP status codes + `x-ratelimit-*` / `Retry-After` headers.
//!
//! **Safety**: requires `--i-understand-this-hits-live-polymarket` and
//! `POLYMARKET_API_KEY` to be set. The API key is only used as a request
//! header; a throwaway / revoked key is preferred.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context};
use clap::Args as ClapArgs;
use hdrhistogram::Histogram;
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::{Request, StatusCode};
use hyper_util::client::legacy::{connect::HttpConnector, Client};
use hyper_util::rt::TokioExecutor;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::report::{print_json, quantiles_ms, Quantiles};

const DEFAULT_URL: &str = "https://clob.polymarket.com/order";

#[derive(ClapArgs, Debug)]
pub struct Args {
    /// Target requests-per-second.
    #[arg(long, default_value_t = 5.0)]
    pub rps: f64,
    /// Total probe duration (seconds).
    #[arg(long, default_value_t = 10)]
    pub duration_secs: u64,
    /// Override target URL (default: Polymarket CLOB /order).
    #[arg(long, default_value = DEFAULT_URL)]
    pub url: String,
    /// Required acknowledgement this hits live infrastructure.
    #[arg(long = "i-understand-this-hits-live-polymarket")]
    pub ack_live: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Report {
    pub url: String,
    pub rps_target: f64,
    pub rps_achieved: f64,
    pub duration_secs: f64,
    pub total_requests: u64,
    pub status_counts: BTreeMap<String, u64>,
    pub first_429_at_request: Option<u64>,
    pub retry_after_observed: Vec<String>,
    pub ratelimit_headers_seen: BTreeMap<String, String>,
    pub response_time: Quantiles,
}

struct Sample {
    status: StatusCode,
    elapsed_us: u64,
    retry_after: Option<String>,
    ratelimit_headers: Vec<(String, String)>,
}

pub async fn run(a: Args) -> anyhow::Result<()> {
    if !a.ack_live {
        return Err(anyhow!(
            "refusing to run: pass --i-understand-this-hits-live-polymarket to confirm"
        ));
    }
    if a.rps <= 0.0 || a.duration_secs == 0 {
        return Err(anyhow!("--rps and --duration-secs must be > 0"));
    }
    let api_key = std::env::var("POLYMARKET_API_KEY").context(
        "POLYMARKET_API_KEY env var required (a throwaway/revoked key is fine — used only as header)",
    )?;

    let tls = hyper_rustls::HttpsConnectorBuilder::new()
        .with_webpki_roots()
        .https_or_http()
        .enable_http1()
        .enable_http2()
        .build();
    let client: Client<_, Full<Bytes>> =
        Client::builder(TokioExecutor::new()).build(tls);

    let total: u64 = ((a.rps * a.duration_secs as f64).ceil() as u64).max(1);
    let interval = Duration::from_secs_f64(1.0 / a.rps);
    let start = Instant::now();
    let samples: Arc<Mutex<Vec<Sample>>> = Arc::new(Mutex::new(Vec::with_capacity(total as usize)));

    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let mut handles = Vec::with_capacity(total as usize);
    for _ in 0..total {
        ticker.tick().await;
        let client = client.clone();
        let url = a.url.clone();
        let api_key = api_key.clone();
        let samples = samples.clone();
        handles.push(tokio::spawn(async move {
            let s = single_request(&client, &url, &api_key).await;
            samples.lock().await.push(s);
        }));
    }
    for h in handles {
        let _ = h.await;
    }
    let elapsed = start.elapsed();

    let samples = Arc::try_unwrap(samples)
        .map_err(|_| anyhow!("sample Arc still shared"))?
        .into_inner();

    let mut status_counts: BTreeMap<String, u64> = BTreeMap::new();
    let mut first_429: Option<u64> = None;
    let mut retry_after_observed: Vec<String> = Vec::new();
    let mut ratelimit_headers_seen: BTreeMap<String, String> = BTreeMap::new();
    let mut hist = Histogram::<u64>::new(3)?;
    for (i, s) in samples.iter().enumerate() {
        *status_counts
            .entry(s.status.as_u16().to_string())
            .or_insert(0) += 1;
        if s.status == StatusCode::TOO_MANY_REQUESTS && first_429.is_none() {
            first_429 = Some(i as u64);
        }
        if let Some(ra) = &s.retry_after {
            if !retry_after_observed.contains(ra) {
                retry_after_observed.push(ra.clone());
            }
        }
        for (k, v) in &s.ratelimit_headers {
            ratelimit_headers_seen.insert(k.clone(), v.clone());
        }
        let _ = hist.record(s.elapsed_us.max(1));
    }

    let total_requests = samples.len() as u64;
    let rps_achieved = total_requests as f64 / elapsed.as_secs_f64().max(1e-9);
    let report = Report {
        url: a.url,
        rps_target: a.rps,
        rps_achieved,
        duration_secs: elapsed.as_secs_f64(),
        total_requests,
        status_counts,
        first_429_at_request: first_429,
        retry_after_observed,
        ratelimit_headers_seen,
        response_time: quantiles_ms(&hist),
    };
    print_json(&report)
}

async fn single_request(
    client: &Client<
        hyper_rustls::HttpsConnector<HttpConnector>,
        Full<Bytes>,
    >,
    url: &str,
    api_key: &str,
) -> Sample {
    // Intentionally-invalid order payload: real shape, bad signature. Server
    // must still pass rate-limit layer (which is what we measure) before
    // rejecting on signature validation.
    let body = serde_json::json!({
        "order": {
            "salt": 0,
            "maker": "0x0000000000000000000000000000000000000000",
            "signer": "0x0000000000000000000000000000000000000000",
            "taker": "0x0000000000000000000000000000000000000000",
            "tokenId": "0",
            "makerAmount": "0",
            "takerAmount": "0",
            "expiration": "0",
            "nonce": "0",
            "feeRateBps": "0",
            "side": "BUY",
            "signatureType": 0,
            "signature": "0x00"
        },
        "owner": "blink-probe",
        "orderType": "GTC"
    })
    .to_string();

    let req = Request::post(url)
        .header("content-type", "application/json")
        .header("user-agent", "blink-probe/ratelimit")
        .header("polymarket-api-key", api_key)
        .body(Full::<Bytes>::from(body));
    let req = match req {
        Ok(r) => r,
        Err(_) => {
            return Sample {
                status: StatusCode::from_u16(0).unwrap_or(StatusCode::BAD_REQUEST),
                elapsed_us: 0,
                retry_after: None,
                ratelimit_headers: vec![],
            }
        }
    };

    let t0 = Instant::now();
    let resp = client.request(req).await;
    let elapsed_us = t0.elapsed().as_micros() as u64;

    match resp {
        Ok(r) => {
            let status = r.status();
            let headers = r.headers().clone();
            let retry_after = headers
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            let mut ratelimit_headers = Vec::new();
            for (k, v) in headers.iter() {
                let name = k.as_str().to_ascii_lowercase();
                if name.starts_with("x-ratelimit") || name == "ratelimit" {
                    if let Ok(s) = v.to_str() {
                        ratelimit_headers.push((name, s.to_string()));
                    }
                }
            }
            // Drain body (ignore errors).
            let _ = r.into_body().collect().await;
            Sample {
                status,
                elapsed_us,
                retry_after,
                ratelimit_headers,
            }
        }
        Err(_) => Sample {
            status: StatusCode::BAD_GATEWAY,
            elapsed_us,
            retry_after: None,
            ratelimit_headers: vec![],
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_roundtrip() {
        let r = Report {
            url: "https://example/order".to_string(),
            rps_target: 5.0,
            rps_achieved: 4.9,
            duration_secs: 10.0,
            total_requests: 49,
            status_counts: [("429".to_string(), 49u64)].into_iter().collect(),
            first_429_at_request: Some(0),
            retry_after_observed: vec!["1".to_string()],
            ratelimit_headers_seen: [(
                "x-ratelimit-remaining".to_string(),
                "0".to_string(),
            )]
            .into_iter()
            .collect(),
            response_time: Quantiles::default(),
        };
        let s = serde_json::to_string(&r).unwrap();
        let back: Report = serde_json::from_str(&s).unwrap();
        assert_eq!(back.total_requests, 49);
        assert_eq!(back.first_429_at_request, Some(0));
    }
}
