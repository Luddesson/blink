//! `MockClobServer` — misbehaving hyper 1 HTTP/2 + rustls server.
//!
//! Supports four primitives:
//!
//! * Serve a configurable HTTP status + body (default `200
//!   {"success":true, "orderID":"mock"}`).
//! * Fail a streak of N requests with a status code (5xx / 429) then
//!   return to normal.
//! * On the Nth request, accept the headers then hard-reset the TCP
//!   socket so the client sees a stream error mid-POST.
//! * Honour `Retry-After` by emitting the header when the faulted
//!   status is 429.
//!
//! The server is bound on `127.0.0.1:0`; callers read the port from
//! the returned handle and hand the TLS trust cert to their
//! `blink-h2` client.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use parking_lot::Mutex;
use rustls::pki_types::CertificateDer;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

use super::tls::{gen_cert, server_config, TestCert};

/// Injectable misbehaviour.
#[derive(Debug, Clone)]
pub struct MockClobBehaviour {
    /// Streak of `fail_streak` requests that return `fail_status`
    /// (and optionally `fail_retry_after_secs` for 429s). Decrements
    /// on each hit, then falls through to the success path.
    pub fail_streak: u32,
    /// HTTP status for the streak.
    pub fail_status: u16,
    /// Body for the failed responses.
    pub fail_body: Bytes,
    /// Optional `Retry-After` (seconds) header on failures.
    pub fail_retry_after_secs: Option<u32>,
    /// If `Some(n)`, on the n-th accepted *connection* (1-indexed)
    /// accept the TCP, read headers, then close the socket before
    /// writing a response. Models the "connection_reset_mid_post"
    /// scenario.
    pub reset_on_nth_conn: Option<u32>,
    /// Default success body.
    pub ok_body: Bytes,
}

impl Default for MockClobBehaviour {
    fn default() -> Self {
        Self {
            fail_streak: 0,
            fail_status: 500,
            fail_body: Bytes::from_static(b"{\"error\":\"mock failure\"}"),
            fail_retry_after_secs: None,
            reset_on_nth_conn: None,
            ok_body: Bytes::from_static(b"{\"success\":true,\"orderID\":\"mock-0x01\"}"),
        }
    }
}

/// Running mock handle.
pub struct MockClobServer {
    /// Bound address.
    pub addr: SocketAddr,
    /// DER-encoded cert (client must add to its trust store).
    pub trust_cert: CertificateDer<'static>,
    /// Shared knobs.
    pub behaviour: Arc<Mutex<MockClobBehaviour>>,
    /// Total accepted connections.
    pub conns: Arc<AtomicU64>,
    /// Total requests received.
    pub requests: Arc<AtomicU64>,
    /// Total successful responses served.
    pub oks: Arc<AtomicU64>,
    /// Total failed responses served.
    pub fails: Arc<AtomicU64>,
    shutdown: Option<oneshot::Sender<()>>,
}

impl MockClobServer {
    /// Spawn the server with `behaviour` on a random port.
    pub async fn spawn(behaviour: MockClobBehaviour) -> Self {
        let TestCert { cert, key } = gen_cert();
        let srv_cfg = server_config(cert.clone(), key);

        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
        let addr = listener.local_addr().expect("local_addr");
        let (tx, mut rx) = oneshot::channel::<()>();

        let beh = Arc::new(Mutex::new(behaviour));
        let conns = Arc::new(AtomicU64::new(0));
        let requests = Arc::new(AtomicU64::new(0));
        let oks = Arc::new(AtomicU64::new(0));
        let fails = Arc::new(AtomicU64::new(0));
        let conn_counter = Arc::new(AtomicU32::new(0));

        let acceptor = tokio_rustls::TlsAcceptor::from(srv_cfg);

        let beh_cl = beh.clone();
        let conns_cl = conns.clone();
        let reqs_cl = requests.clone();
        let oks_cl = oks.clone();
        let fails_cl = fails.clone();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut rx => return,
                    accept = listener.accept() => {
                        let (tcp, _peer) = match accept {
                            Ok(v) => v,
                            Err(_) => continue,
                        };
                        let _ = tcp.set_nodelay(true);
                        conns_cl.fetch_add(1, Ordering::Relaxed);
                        let nth = conn_counter.fetch_add(1, Ordering::Relaxed) + 1;
                        let reset_nth = beh_cl.lock().reset_on_nth_conn;

                        let acceptor = acceptor.clone();
                        let beh = beh_cl.clone();
                        let reqs = reqs_cl.clone();
                        let oks = oks_cl.clone();
                        let fails = fails_cl.clone();

                        tokio::spawn(async move {
                            let tls = match acceptor.accept(tcp).await {
                                Ok(t) => t,
                                Err(_) => return,
                            };

                            if Some(nth) == reset_nth {
                                // Accept TLS, then drop immediately —
                                // the client will see a stream-level
                                // H2 error.
                                drop(tls);
                                return;
                            }

                            let io = TokioIo::new(tls);
                            let beh2 = beh.clone();
                            let svc = service_fn(move |req: Request<Incoming>| {
                                let beh = beh2.clone();
                                let reqs = reqs.clone();
                                let oks = oks.clone();
                                let fails = fails.clone();
                                async move {
                                    // Drain body so the peer observes
                                    // the exchange complete.
                                    let _ = req.into_body().collect().await;
                                    reqs.fetch_add(1, Ordering::Relaxed);
                                    let (status, body, retry_after) = {
                                        let mut g = beh.lock();
                                        if g.fail_streak > 0 {
                                            g.fail_streak -= 1;
                                            let ra = g.fail_retry_after_secs;
                                            (g.fail_status, g.fail_body.clone(), ra)
                                        } else {
                                            (200u16, g.ok_body.clone(), None)
                                        }
                                    };
                                    let mut builder = Response::builder()
                                        .status(status)
                                        .header("content-type", "application/json");
                                    if let Some(ra) = retry_after {
                                        builder = builder.header("retry-after", ra.to_string());
                                    }
                                    if status == 200 {
                                        oks.fetch_add(1, Ordering::Relaxed);
                                    } else {
                                        fails.fetch_add(1, Ordering::Relaxed);
                                    }
                                    Ok::<_, hyper::Error>(
                                        builder.body(Full::new(body)).unwrap(),
                                    )
                                }
                            });
                            let srv = hyper::server::conn::http2::Builder::new(TokioExecutor::new())
                                .timer(TokioTimer::new())
                                .serve_connection(io, svc);
                            let _ = srv.await;
                        });
                    }
                }
            }
        });

        Self {
            addr,
            trust_cert: cert,
            behaviour: beh,
            conns,
            requests,
            oks,
            fails,
            shutdown: Some(tx),
        }
    }

    /// Shut down the accept loop. Idempotent.
    pub fn shutdown(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

impl Drop for MockClobServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}
