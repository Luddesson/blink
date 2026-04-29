//! R-7: Cloudflare submit-path latency probe.
//!
//! Resolves `clob.polymarket.com` A records, then fires 1000 HTTP/2 GETs
//! on a single persistent connection to measure body-complete latency
//! distribution + Cloudflare edge metadata (`cf-ray`, `cf-cache-status`).
//!
//! Endpoint choice: `GET /` — we're measuring **network** (Cloudflare
//! edge + TLS + HTTP/2 framing), not application correctness. Any status
//! code is accepted. A 404 with normal `cf-ray` is just as useful as 200.

use std::net::IpAddr;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context};
use clap::Args as ClapArgs;
use hdrhistogram::Histogram;
use hickory_resolver::config::{ResolverConfig, ResolverOpts};
use hickory_resolver::TokioAsyncResolver;
use http_body_util::{BodyExt, Empty};
use hyper::body::Bytes;
use hyper::Request;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use serde::{Deserialize, Serialize};

use crate::report::{print_json, quantiles_ms, Quantiles};

const DEFAULT_HOST: &str = "clob.polymarket.com";
const DEFAULT_PATH: &str = "/";
const DEFAULT_ITERS: u64 = 1000;

#[derive(ClapArgs, Debug)]
pub struct Args {
    /// Hostname to probe.
    #[arg(long, default_value = DEFAULT_HOST)]
    pub host: String,
    /// Path to GET. Default `/`; status code is informational.
    #[arg(long, default_value = DEFAULT_PATH)]
    pub path: String,
    /// Number of HTTP/2 requests on the persistent connection.
    #[arg(long, default_value_t = DEFAULT_ITERS)]
    pub iters: u64,
    /// Per-request timeout (milliseconds).
    #[arg(long, default_value_t = 5_000)]
    pub timeout_ms: u64,
    /// Free-form tag recorded in report (e.g. operator region).
    #[arg(long)]
    pub region: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Report {
    pub host: String,
    pub path: String,
    pub region_tag: Option<String>,
    pub dns_a_records: Vec<String>,
    pub iters: u64,
    pub succeeded: u64,
    pub failed: u64,
    pub status_counts: std::collections::BTreeMap<String, u64>,
    pub cf_ray_samples: Vec<String>,
    pub cf_cache_status_samples: Vec<String>,
    pub server_header_samples: Vec<String>,
    pub inferred_anycast_pops: Vec<String>,
    pub body_complete: Quantiles,
}

pub async fn run(a: Args) -> anyhow::Result<()> {
    if a.iters == 0 {
        return Err(anyhow!("--iters must be > 0"));
    }
    let resolver = TokioAsyncResolver::tokio(ResolverConfig::default(), ResolverOpts::default());
    let dns_a_records: Vec<String> = resolver
        .ipv4_lookup(a.host.clone())
        .await
        .with_context(|| format!("resolving A for {}", a.host))?
        .iter()
        .map(|ip| IpAddr::V4(ip.0).to_string())
        .collect();

    let tls = hyper_rustls::HttpsConnectorBuilder::new()
        .with_webpki_roots()
        .https_only()
        .enable_http2()
        .build();
    let client: Client<_, Empty<Bytes>> = Client::builder(TokioExecutor::new())
        .http2_only(true)
        .pool_idle_timeout(Duration::from_secs(60))
        .build(tls);

    let url = format!("https://{}{}", a.host, a.path);
    let mut hist = Histogram::<u64>::new(3)?;
    let mut status_counts: std::collections::BTreeMap<String, u64> =
        std::collections::BTreeMap::new();
    let mut cf_ray_samples: Vec<String> = Vec::new();
    let mut cf_cache: Vec<String> = Vec::new();
    let mut server_hdrs: Vec<String> = Vec::new();
    let mut pops: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut succeeded = 0u64;
    let mut failed = 0u64;

    // Warm-up: first request opens the connection (not counted).
    let warmup = Request::get(&url)
        .header("user-agent", "blink-probe/cloudflare")
        .body(Empty::<Bytes>::new())?;
    if let Ok(r) = client.request(warmup).await {
        let _ = r.into_body().collect().await;
    }

    for _ in 0..a.iters {
        let req = Request::get(&url)
            .header("user-agent", "blink-probe/cloudflare")
            .body(Empty::<Bytes>::new())?;
        let t0 = Instant::now();
        let fut = client.request(req);
        let resp = tokio::time::timeout(Duration::from_millis(a.timeout_ms), fut).await;
        match resp {
            Ok(Ok(r)) => {
                let status = r.status();
                let headers = r.headers().clone();
                let body_res = r.into_body().collect().await;
                let dt_us = t0.elapsed().as_micros() as u64;
                if body_res.is_err() {
                    failed += 1;
                    continue;
                }
                succeeded += 1;
                let _ = hist.record(dt_us.max(1));
                *status_counts
                    .entry(status.as_u16().to_string())
                    .or_insert(0) += 1;
                if let Some(v) = headers.get("cf-ray").and_then(|h| h.to_str().ok()) {
                    if cf_ray_samples.len() < 10 {
                        cf_ray_samples.push(v.to_string());
                    }
                    // `cf-ray` format: `<hex>-<POP>` where POP is the 3-letter airport code.
                    if let Some((_, pop)) = v.rsplit_once('-') {
                        pops.insert(pop.to_string());
                    }
                }
                if let Some(v) = headers.get("cf-cache-status").and_then(|h| h.to_str().ok()) {
                    if cf_cache.len() < 10 {
                        cf_cache.push(v.to_string());
                    }
                }
                if let Some(v) = headers.get("server").and_then(|h| h.to_str().ok()) {
                    if server_hdrs.len() < 10 {
                        server_hdrs.push(v.to_string());
                    }
                }
            }
            _ => failed += 1,
        }
    }

    let report = Report {
        host: a.host,
        path: a.path,
        region_tag: a.region,
        dns_a_records,
        iters: a.iters,
        succeeded,
        failed,
        status_counts,
        cf_ray_samples,
        cf_cache_status_samples: cf_cache,
        server_header_samples: server_hdrs,
        inferred_anycast_pops: pops.into_iter().collect(),
        body_complete: quantiles_ms(&hist),
    };
    print_json(&report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_roundtrip() {
        let r = Report {
            host: "clob.polymarket.com".to_string(),
            path: "/".to_string(),
            region_tag: Some("iad".to_string()),
            dns_a_records: vec!["1.2.3.4".to_string()],
            iters: 1000,
            succeeded: 1000,
            failed: 0,
            status_counts: [("200".to_string(), 1000u64)].into_iter().collect(),
            cf_ray_samples: vec!["abc123-IAD".to_string()],
            cf_cache_status_samples: vec!["DYNAMIC".to_string()],
            server_header_samples: vec!["cloudflare".to_string()],
            inferred_anycast_pops: vec!["IAD".to_string()],
            body_complete: Quantiles::default(),
        };
        let s = serde_json::to_string(&r).unwrap();
        let back: Report = serde_json::from_str(&s).unwrap();
        assert_eq!(back.inferred_anycast_pops, vec!["IAD".to_string()]);
    }
}
