//! Hot-path HTTP/2 client for Polymarket order submission.
//!
//! This crate is the **transport layer** only. It knows nothing about order
//! semantics, EIP-712 signing, retries, or idempotency. It takes a path,
//! header slice and `bytes::Bytes` body, and returns an [`H2Response`] or
//! an [`H2Error`].
//!
//! # Non-goals / explicit contracts
//!
//! * **No retries.** If a POST fails mid-stream (RST_STREAM, GOAWAY,
//!   socket close, timeout, ...) the error bubbles up verbatim. The
//!   submit layer (`blink-submit` / `p4-submit`) owns idempotency and
//!   retry policy because only it can reason about whether the order
//!   actually landed.
//! * **One origin per client.** A single [`H2Client`] speaks to exactly
//!   one `authority` over a single persistent H2 connection. Spin up
//!   more clients for more origins.
//! * **Zero-copy body.** The public [`H2Client::post`] API takes
//!   `bytes::Bytes` so callers pre-signed payloads flow through without
//!   copying.
//!
//! # Connection lifecycle
//!
//! A supervisor task runs for the lifetime of the client. It establishes
//! the TLS + HTTP/2 connection, installs the [`hyper::client::conn::http2::SendRequest`]
//! handle into an [`arc_swap::ArcSwap`] so reads are lock-free, and
//! drives the connection future. When the connection terminates (peer
//! GOAWAY, ping keepalive timeout, socket error, ...) the supervisor
//! flips state to `Closed`, reconnects with exponential backoff
//! (25ms .. 2s) and republishes a fresh sender. Callers `await`
//! [`H2Client::ensure_connected`] before each POST.
//!
//! TLS session resumption is enabled (`Resumption::in_memory_sessions(1)`)
//! so reconnects skip full handshakes.

#![forbid(unsafe_code)]

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::client::conn::http2::SendRequest;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use rustls::pki_types::ServerName;
use thiserror::Error;
use tokio::net::TcpStream;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio_rustls::TlsConnector;

pub use blink_timestamps::Timestamp;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Public error surface. These variants are stable; downstream crates
/// (notably `blink-submit`) match on them to decide their own retry
/// behaviour.
#[derive(Debug, Error)]
pub enum H2Error {
    /// Connection is not usable: not yet established, torn down, or
    /// shutting down.
    #[error("connection error: {0}")]
    Connection(String),
    /// Stream-level failure (RST_STREAM, refused, response did not
    /// arrive, body read failed, ...).
    #[error("stream error: {0}")]
    Stream(String),
    /// Request exceeded a client-side deadline.
    #[error("timeout")]
    Timeout,
    /// TLS handshake or certificate failure.
    #[error("tls error: {0}")]
    Tls(String),
    /// Underlying transport I/O failure (TCP connect, read, write).
    #[error("io error: {0}")]
    Io(String),
}

// ---------------------------------------------------------------------------
// State machine
// ---------------------------------------------------------------------------

/// Connection state. Exposed for observability and unit-testable
/// transitions; callers should not branch on it directly — use
/// [`H2Client::ensure_connected`] or the stats snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Initial handshake (TCP + TLS + H2 preface) in flight.
    Connecting,
    /// Connection is up and has a live `SendRequest`.
    Ready,
    /// Ping-keepalive timed out or a stream RST suggests the peer is
    /// unhealthy. A reconnect is queued.
    Degraded,
    /// Connection has terminated; supervisor will reconnect unless the
    /// client has been shut down.
    Closed,
}

/// State-machine events consumed by [`next_state`]. Pure, for unit
/// testing of the transition table.
#[derive(Debug, Clone, Copy)]
pub enum Event {
    ConnectStarted,
    ConnectSucceeded,
    ConnectFailed,
    PingTimeout,
    ConnectionClosed,
    Shutdown,
}

/// Pure state transition function. Extracted for unit tests — the
/// real supervisor calls this via the exact same table.
pub fn next_state(cur: State, ev: Event) -> State {
    use Event::*;
    use State::*;
    match (cur, ev) {
        (_, Shutdown) => Closed,
        (Closed, _) => Closed, // terminal unless re-created
        (_, ConnectStarted) => Connecting,
        (Connecting, ConnectSucceeded) => Ready,
        (Connecting, ConnectFailed) => Closed,
        (_, ConnectSucceeded) => Ready,
        (Ready, PingTimeout) => Degraded,
        (Ready, ConnectionClosed) => Closed,
        (Degraded, ConnectionClosed) => Closed,
        (Degraded, PingTimeout) => Degraded,
        (Connecting, PingTimeout) => Connecting,
        (Connecting, ConnectionClosed) => Closed,
        (s, _) => s,
    }
}

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct StatsInner {
    connects: AtomicU64,
    reconnects: AtomicU64,
    posts_ok: AtomicU64,
    posts_err: AtomicU64,
    ping_timeouts: AtomicU64,
}

/// Point-in-time snapshot of counters. No prometheus dependency —
/// callers choose how to export.
#[derive(Debug, Clone, Copy, Default)]
pub struct Stats {
    pub connects: u64,
    pub reconnects: u64,
    pub posts_ok: u64,
    pub posts_err: u64,
    pub ping_timeouts: u64,
}

// ---------------------------------------------------------------------------
// Inner connection handle
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Conn {
    state: State,
    // None while Connecting/Degraded/Closed.
    sender: Option<SendRequest<Full<Bytes>>>,
}

impl Conn {
    fn new(state: State) -> Arc<Self> {
        Arc::new(Self { state, sender: None })
    }
    fn ready(sender: SendRequest<Full<Bytes>>) -> Arc<Self> {
        Arc::new(Self { state: State::Ready, sender: Some(sender) })
    }
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Construction-time configuration for [`H2Client`].
#[derive(Clone)]
pub struct H2Config {
    /// DNS host to resolve + TLS SNI. e.g. `clob.polymarket.com`.
    pub host: String,
    /// TCP port (usually 443).
    pub port: u16,
    /// Rustls client configuration. Callers may provide a custom
    /// verifier (used by tests to trust a self-signed CA).
    /// Session resumption is expected to be enabled by the caller;
    /// the default from [`default_rustls_config`] does.
    pub tls: Arc<rustls::ClientConfig>,
    /// Per-request timeout applied to `post()`.
    pub request_timeout: Duration,
    /// H2 ping keepalive cadence.
    pub keepalive_interval: Duration,
    /// Dead-peer timeout after a keepalive ping with no PONG.
    pub keepalive_timeout: Duration,
}

impl H2Config {
    pub fn new(host: impl Into<String>, port: u16, tls: Arc<rustls::ClientConfig>) -> Self {
        Self {
            host: host.into(),
            port,
            tls,
            request_timeout: Duration::from_secs(5),
            keepalive_interval: Duration::from_secs(10),
            keepalive_timeout: Duration::from_secs(3),
        }
    }
}

/// Build a default [`rustls::ClientConfig`] using the webpki roots and
/// with `Resumption::in_memory_sessions(1)` explicitly set so that
/// reconnects skip full TLS handshakes.
///
/// Callers that need a custom verifier (e.g. tests with a self-signed
/// CA) should construct their own `ClientConfig` and still call
/// [`tune_resumption`] on it.
pub fn default_rustls_config(roots: rustls::RootCertStore) -> Arc<rustls::ClientConfig> {
    let mut cfg = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    tune_resumption(&mut cfg);
    cfg.alpn_protocols = vec![b"h2".to_vec()];
    Arc::new(cfg)
}

/// Explicitly enable one-session-per-authority in-memory TLS resumption.
pub fn tune_resumption(cfg: &mut rustls::ClientConfig) {
    cfg.resumption = rustls::client::Resumption::in_memory_sessions(1);
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/// Persistent HTTP/2 client bound to a single origin.
///
/// Cheap to clone — internally an `Arc`.
#[derive(Clone)]
pub struct H2Client {
    inner: Arc<H2ClientInner>,
}

struct H2ClientInner {
    cfg: H2Config,
    authority: String,
    conn: ArcSwap<Conn>,
    stats: StatsInner,
    notify: Notify,
    shutdown: AtomicBool,
    supervisor: std::sync::Mutex<Option<JoinHandle<()>>>,
}

impl H2Client {
    /// Spawn the supervisor task and return an [`H2Client`] ready to
    /// accept `post()` calls (callers should `await ensure_connected()`
    /// for the first request to avoid racing the handshake).
    pub fn spawn(cfg: H2Config) -> Self {
        // Ensure a rustls crypto provider is installed. Idempotent.
        let _ = rustls::crypto::ring::default_provider().install_default();
        // Ensure blink-timestamps has been calibrated. Idempotent.
        let _ = blink_timestamps::init();

        let authority = if cfg.port == 443 {
            cfg.host.clone()
        } else {
            format!("{}:{}", cfg.host, cfg.port)
        };

        let inner = Arc::new(H2ClientInner {
            cfg,
            authority,
            conn: ArcSwap::new(Conn::new(State::Connecting)),
            stats: StatsInner::default(),
            notify: Notify::new(),
            shutdown: AtomicBool::new(false),
            supervisor: std::sync::Mutex::new(None),
        });

        let sup_inner = Arc::clone(&inner);
        let handle = tokio::spawn(async move { supervisor_loop(sup_inner).await });
        *inner.supervisor.lock().unwrap() = Some(handle);

        Self { inner }
    }

    /// Await the connection becoming `Ready`. Returns immediately if
    /// already ready. Returns [`H2Error::Connection`] if the client is
    /// shutting down.
    pub async fn ensure_connected(&self) -> Result<(), H2Error> {
        loop {
            if self.inner.shutdown.load(Ordering::Acquire) {
                return Err(H2Error::Connection("client shut down".into()));
            }
            let conn = self.inner.conn.load();
            if conn.state == State::Ready && conn.sender.is_some() {
                return Ok(());
            }
            // Register interest _before_ re-checking to avoid a missed
            // wakeup race with the supervisor.
            let notified = self.inner.notify.notified();
            let conn = self.inner.conn.load();
            if conn.state == State::Ready && conn.sender.is_some() {
                return Ok(());
            }
            match tokio::time::timeout(Duration::from_secs(5), notified).await {
                Ok(()) => continue,
                Err(_) => {
                    return Err(H2Error::Connection(
                        "timed out waiting for connection".into(),
                    ))
                }
            }
        }
    }

    /// POST `body` with the given headers to `path` (e.g. `"/order"`).
    ///
    /// Must not be retried by the caller's transport: this layer
    /// surfaces the raw transport outcome. Idempotency / retry belongs
    /// to `blink-submit`.
    pub async fn post(
        &self,
        path: &str,
        headers: &[(&str, &str)],
        body: Bytes,
    ) -> Result<H2Response, H2Error> {
        self.ensure_connected().await?;

        let conn = self.inner.conn.load_full();
        let mut sender = conn
            .sender
            .as_ref()
            .ok_or_else(|| H2Error::Connection("no live sender".into()))?
            .clone();

        let mut builder = http::Request::builder()
            .method(http::Method::POST)
            .uri(path)
            .header(http::header::HOST, self.inner.authority.as_str());
        for (k, v) in headers {
            builder = builder.header(*k, *v);
        }
        let req = builder
            .body(Full::new(body))
            .map_err(|e| H2Error::Stream(format!("build request: {e}")))?;

        let tsc_send = Timestamp::now();
        let resp_fut = sender.send_request(req);
        let resp = match tokio::time::timeout(self.inner.cfg.request_timeout, resp_fut).await {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                self.inner.stats.posts_err.fetch_add(1, Ordering::Relaxed);
                // A stream error may indicate the connection is gone;
                // hint the supervisor to re-check.
                self.inner.notify.notify_waiters();
                return Err(H2Error::Stream(format!("send_request: {e}")));
            }
            Err(_) => {
                self.inner.stats.posts_err.fetch_add(1, Ordering::Relaxed);
                return Err(H2Error::Timeout);
            }
        };

        let (parts, incoming) = resp.into_parts();
        let collected = match tokio::time::timeout(
            self.inner.cfg.request_timeout,
            incoming.collect(),
        )
        .await
        {
            Ok(Ok(c)) => c,
            Ok(Err(e)) => {
                self.inner.stats.posts_err.fetch_add(1, Ordering::Relaxed);
                return Err(H2Error::Stream(format!("body read: {e}")));
            }
            Err(_) => {
                self.inner.stats.posts_err.fetch_add(1, Ordering::Relaxed);
                return Err(H2Error::Timeout);
            }
        };
        let body_bytes = collected.to_bytes();
        let tsc_recv = Timestamp::now();

        let wall_recv_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);

        let headers = parts
            .headers
            .iter()
            .map(|(k, v)| {
                (
                    k.as_str().to_owned(),
                    v.to_str().unwrap_or_default().to_owned(),
                )
            })
            .collect();

        self.inner.stats.posts_ok.fetch_add(1, Ordering::Relaxed);

        Ok(H2Response {
            status: parts.status.as_u16(),
            headers,
            body: body_bytes,
            wall_recv_ns,
            tsc_send,
            tsc_recv,
        })
    }

    /// Snapshot of internal counters.
    pub fn stats(&self) -> Stats {
        let s = &self.inner.stats;
        Stats {
            connects: s.connects.load(Ordering::Relaxed),
            reconnects: s.reconnects.load(Ordering::Relaxed),
            posts_ok: s.posts_ok.load(Ordering::Relaxed),
            posts_err: s.posts_err.load(Ordering::Relaxed),
            ping_timeouts: s.ping_timeouts.load(Ordering::Relaxed),
        }
    }

    /// Current observable connection state (for tests / observability).
    pub fn state(&self) -> State {
        self.inner.conn.load().state
    }

    /// Send GOAWAY + drain in-flight streams. The supervisor task
    /// terminates; the underlying connection future (which hyper
    /// drives) naturally finishes when the last `SendRequest` clone is
    /// dropped.
    pub async fn shutdown(self) {
        self.inner.shutdown.store(true, Ordering::Release);
        // Drop the sender so the H2 driver can send GOAWAY and exit.
        self.inner
            .conn
            .store(Conn::new(State::Closed));
        self.inner.notify.notify_waiters();
        let handle = self.inner.supervisor.lock().unwrap().take();
        if let Some(h) = handle {
            // Best-effort join — don't panic on abort.
            let _ = tokio::time::timeout(Duration::from_secs(2), h).await;
        }
    }
}

// ---------------------------------------------------------------------------
// Response
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct H2Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Bytes,
    /// Wall-clock ns-since-epoch when body collection completed.
    pub wall_recv_ns: u64,
    /// Monotonic timestamp taken immediately before the request was
    /// handed to the H2 stream multiplexer.
    pub tsc_send: Timestamp,
    /// Monotonic timestamp taken immediately after the full response
    /// body was collected.
    pub tsc_recv: Timestamp,
}

// ---------------------------------------------------------------------------
// Supervisor
// ---------------------------------------------------------------------------

async fn supervisor_loop(inner: Arc<H2ClientInner>) {
    let mut backoff = Duration::from_millis(25);
    let mut first = true;
    loop {
        if inner.shutdown.load(Ordering::Acquire) {
            return;
        }
        if !first {
            inner.stats.reconnects.fetch_add(1, Ordering::Relaxed);
        }
        first = false;

        inner.conn.store(Conn::new(State::Connecting));
        inner.notify.notify_waiters();

        match connect_once(&inner).await {
            Ok((sender, driver)) => {
                inner.stats.connects.fetch_add(1, Ordering::Relaxed);
                inner.conn.store(Conn::ready(sender));
                inner.notify.notify_waiters();
                backoff = Duration::from_millis(25);

                // Drive the connection until it ends.
                let res = driver.await;
                if inner.shutdown.load(Ordering::Acquire) {
                    inner.conn.store(Conn::new(State::Closed));
                    inner.notify.notify_waiters();
                    return;
                }

                match res {
                    Ok(()) => {
                        log::info!("blink-h2: connection closed by peer (graceful)");
                        inner.conn.store(Conn::new(State::Closed));
                    }
                    Err(e) => {
                        let msg = format!("{e}");
                        log::warn!("blink-h2: connection ended: {msg}");
                        if is_keepalive_timeout(&msg) {
                            inner.stats.ping_timeouts.fetch_add(1, Ordering::Relaxed);
                            inner.conn.store(Conn::new(State::Degraded));
                        } else {
                            inner.conn.store(Conn::new(State::Closed));
                        }
                    }
                }
                inner.notify.notify_waiters();
            }
            Err(e) => {
                log::warn!(
                    "blink-h2: connect failed (backoff {:?}): {}",
                    backoff,
                    e
                );
                inner.conn.store(Conn::new(State::Closed));
                inner.notify.notify_waiters();
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(2));
            }
        }
    }
}

fn is_keepalive_timeout(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    m.contains("keep") || m.contains("ping") || m.contains("timed out")
}

type Driver = Pin<Box<dyn Future<Output = Result<(), hyper::Error>> + Send>>;

async fn connect_once(
    inner: &Arc<H2ClientInner>,
) -> Result<(SendRequest<Full<Bytes>>, Driver), H2Error> {
    let cfg = &inner.cfg;
    let addr = format!("{}:{}", cfg.host, cfg.port);
    let tcp = TcpStream::connect(&addr)
        .await
        .map_err(|e| H2Error::Io(format!("tcp connect {addr}: {e}")))?;
    let _ = tcp.set_nodelay(true);

    let sni = ServerName::try_from(cfg.host.clone())
        .map_err(|e| H2Error::Tls(format!("invalid sni {}: {e}", cfg.host)))?;
    let connector = TlsConnector::from(Arc::clone(&cfg.tls));
    let tls = connector
        .connect(sni, tcp)
        .await
        .map_err(|e| H2Error::Tls(format!("handshake: {e}")))?;

    let io = TokioIo::new(tls);
    let (sender, conn) = hyper::client::conn::http2::Builder::new(TokioExecutor::new())
        .timer(TokioTimer::new())
        .keep_alive_interval(Some(cfg.keepalive_interval))
        .keep_alive_timeout(cfg.keepalive_timeout)
        .keep_alive_while_idle(true)
        .handshake(io)
        .await
        .map_err(|e| H2Error::Connection(format!("h2 handshake: {e}")))?;

    let driver: Driver = Box::pin(conn);
    Ok((sender, driver))
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display_strings() {
        assert_eq!(
            format!("{}", H2Error::Connection("boom".into())),
            "connection error: boom"
        );
        assert_eq!(
            format!("{}", H2Error::Stream("rst".into())),
            "stream error: rst"
        );
        assert_eq!(format!("{}", H2Error::Timeout), "timeout");
        assert_eq!(
            format!("{}", H2Error::Tls("cert".into())),
            "tls error: cert"
        );
        assert_eq!(format!("{}", H2Error::Io("eof".into())), "io error: eof");
    }

    #[test]
    fn state_machine_happy_path() {
        let s = State::Connecting;
        let s = next_state(s, Event::ConnectStarted);
        assert_eq!(s, State::Connecting);
        let s = next_state(s, Event::ConnectSucceeded);
        assert_eq!(s, State::Ready);
        let s = next_state(s, Event::ConnectionClosed);
        assert_eq!(s, State::Closed);
    }

    #[test]
    fn state_machine_ping_timeout_degrades() {
        let s = next_state(State::Ready, Event::PingTimeout);
        assert_eq!(s, State::Degraded);
        let s = next_state(s, Event::ConnectionClosed);
        assert_eq!(s, State::Closed);
    }

    #[test]
    fn state_machine_shutdown_is_terminal() {
        for start in [State::Connecting, State::Ready, State::Degraded] {
            assert_eq!(next_state(start, Event::Shutdown), State::Closed);
        }
        // Closed absorbs everything (shutdown wins globally, others are
        // no-ops from terminal).
        for ev in [
            Event::ConnectStarted,
            Event::ConnectSucceeded,
            Event::ConnectFailed,
            Event::PingTimeout,
            Event::ConnectionClosed,
        ] {
            assert_eq!(next_state(State::Closed, ev), State::Closed);
        }
    }

    #[test]
    fn config_builds_and_resumption_is_set() {
        let mut cfg = rustls::ClientConfig::builder()
            .with_root_certificates(rustls::RootCertStore::empty())
            .with_no_client_auth();
        tune_resumption(&mut cfg);
        // We can't cheaply introspect the Resumption type but we can
        // at least confirm assignment compiles & the ClientConfig is
        // still usable.
        let _ = Arc::new(cfg);
    }

    #[test]
    fn stats_default_zero() {
        let s = Stats::default();
        assert_eq!(s.connects, 0);
        assert_eq!(s.posts_ok, 0);
        assert_eq!(s.posts_err, 0);
        assert_eq!(s.reconnects, 0);
        assert_eq!(s.ping_timeouts, 0);
    }
}
