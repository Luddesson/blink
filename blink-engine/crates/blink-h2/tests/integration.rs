//! End-to-end H2 tests against an in-process hyper server secured with
//! a self-signed cert. Verifies:
//! * 100 POSTs land on a single persistent connection (`connects == 1`)
//! * when the server is killed, the client observes a non-Ready state
//!   and reconnects when the server comes back (`reconnects >= 1`).

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use blink_h2::{H2Client, H2Config, State, tune_resumption};
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::net::TcpListener;
use tokio::sync::{oneshot, Notify};

fn gen_cert() -> (CertificateDer<'static>, PrivateKeyDer<'static>) {
    let ca = rcgen::generate_simple_self_signed(vec!["localhost".into()]).expect("rcgen");
    let cert_der = CertificateDer::from(ca.cert.der().to_vec());
    let key_der =
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(ca.key_pair.serialize_der()));
    (cert_der, key_der)
}

fn client_cfg(trust: CertificateDer<'static>) -> Arc<rustls::ClientConfig> {
    let mut roots = rustls::RootCertStore::empty();
    roots.add(trust).expect("add root");
    let mut cfg = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    tune_resumption(&mut cfg);
    cfg.alpn_protocols = vec![b"h2".to_vec()];
    Arc::new(cfg)
}

fn server_cfg(
    cert: CertificateDer<'static>,
    key: PrivateKeyDer<'static>,
) -> Arc<rustls::ServerConfig> {
    let mut cfg = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .expect("server cert");
    cfg.alpn_protocols = vec![b"h2".to_vec()];
    Arc::new(cfg)
}

struct ServerHandle {
    addr: SocketAddr,
    shutdown: Option<oneshot::Sender<()>>,
    #[allow(dead_code)]
    kill_conns: Arc<Notify>,
    hits: Arc<AtomicU64>,
}

async fn spawn_server(
    cert: CertificateDer<'static>,
    key: PrivateKeyDer<'static>,
) -> ServerHandle {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
    let addr = listener.local_addr().unwrap();
    let (tx, mut rx) = oneshot::channel::<()>();
    let hits = Arc::new(AtomicU64::new(0));
    let hits_cl = Arc::clone(&hits);
    let kill_conns = Arc::new(Notify::new());
    let kill_conns_cl = Arc::clone(&kill_conns);
    let acceptor = tokio_rustls::TlsAcceptor::from(server_cfg(cert, key));

    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut rx => { kill_conns_cl.notify_waiters(); return; },
                accept = listener.accept() => {
                    let (tcp, _peer) = match accept {
                        Ok(v) => v,
                        Err(_) => continue,
                    };
                    let _ = tcp.set_nodelay(true);
                    let acceptor = acceptor.clone();
                    let hits = Arc::clone(&hits_cl);
                    let kill = Arc::clone(&kill_conns_cl);
                    tokio::spawn(async move {
                        let tls = match acceptor.accept(tcp).await {
                            Ok(t) => t,
                            Err(_) => return,
                        };
                        let io = TokioIo::new(tls);
                        let svc = service_fn(move |req: Request<Incoming>| {
                            let hits = Arc::clone(&hits);
                            async move {
                                let _ = req.into_body().collect().await;
                                hits.fetch_add(1, Ordering::Relaxed);
                                Ok::<_, hyper::Error>(
                                    Response::builder()
                                        .status(200)
                                        .header("content-type", "application/json")
                                        .body(Full::new(Bytes::from_static(b"{\"ok\":true}")))
                                        .unwrap(),
                                )
                            }
                        });
                        let srv = hyper::server::conn::http2::Builder::new(TokioExecutor::new()).timer(TokioTimer::new())
                            .serve_connection(io, svc);
                        tokio::select! {
                            _ = kill.notified() => {},
                            _ = srv => {},
                        }
                    });
                }
            }
        }
    });

    ServerHandle { addr, shutdown: Some(tx), kill_conns, hits }
}

impl ServerHandle {
    fn kill(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hundred_posts_single_connection() {
    let (cert, key) = gen_cert();
    let cert_client = cert.clone();
    let srv = spawn_server(cert, key).await;

    let mut cfg = H2Config::new("localhost", srv.addr.port(), client_cfg(cert_client));
    cfg.request_timeout = Duration::from_secs(5);
    let client = H2Client::spawn(cfg);

    client.ensure_connected().await.expect("connect");

    for i in 0..100u32 {
        let body = Bytes::from(format!("{{\"n\":{}}}", i));
        let resp = client
            .post("/order", &[("content-type", "application/json")], body)
            .await
            .unwrap_or_else(|e| panic!("post {i}: {e}"));
        assert_eq!(resp.status, 200);
        assert!(resp.tsc_recv.raw() >= resp.tsc_send.raw());
        assert!(!resp.body.is_empty());
    }

    let stats = client.stats();
    assert_eq!(stats.posts_ok, 100, "stats={stats:?}");
    assert_eq!(stats.posts_err, 0, "stats={stats:?}");
    assert_eq!(stats.connects, 1, "should reuse single conn: stats={stats:?}");
    assert_eq!(srv.hits.load(Ordering::Relaxed), 100);

    client.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reconnects_after_server_restart() {
    let (cert, key) = gen_cert();
    let cert_client = cert.clone();

    let mut srv = spawn_server(cert.clone(), key.clone_key()).await;
    let port = srv.addr.port();

    let mut cfg = H2Config::new("localhost", port, client_cfg(cert_client));
    cfg.request_timeout = Duration::from_secs(3);
    cfg.keepalive_interval = Duration::from_millis(200);
    cfg.keepalive_timeout = Duration::from_millis(200);
    let client = H2Client::spawn(cfg);

    client.ensure_connected().await.expect("connect 1");
    let r = client
        .post("/order", &[], Bytes::from_static(b"hi"))
        .await
        .expect("post 1");
    assert_eq!(r.status, 200);

    srv.kill();

    let waited = async {
        for _ in 0..200 {
            if client.state() != State::Ready {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        false
    }
    .await;
    assert!(waited, "client never left Ready after server kill");

    let listener = loop {
        match TcpListener::bind(("127.0.0.1", port)).await {
            Ok(l) => break l,
            Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    };
    let (tx, mut rx) = oneshot::channel::<()>();
    let acceptor = tokio_rustls::TlsAcceptor::from(server_cfg(cert, key));
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut rx => return,
                accept = listener.accept() => {
                    let (tcp, _) = match accept { Ok(v) => v, Err(_) => continue };
                    let _ = tcp.set_nodelay(true);
                    let acceptor = acceptor.clone();
                    tokio::spawn(async move {
                        let tls = match acceptor.accept(tcp).await {
                            Ok(t) => t, Err(_) => return,
                        };
                        let io = TokioIo::new(tls);
                        let svc = service_fn(|req: Request<Incoming>| async move {
                            let _ = req.into_body().collect().await;
                            Ok::<_, hyper::Error>(
                                Response::builder()
                                    .status(200)
                                    .body(Full::new(Bytes::from_static(b"back")))
                                    .unwrap())
                        });
                        let _ = hyper::server::conn::http2::Builder::new(TokioExecutor::new()).timer(TokioTimer::new())
                            .serve_connection(io, svc).await;
                    });
                }
            }
        }
    });

    let mut reconnected = false;
    for _ in 0..200 {
        if client.ensure_connected().await.is_ok() && client.state() == State::Ready {
            reconnected = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(reconnected, "never reconnected; stats={:?}", client.stats());

    let r2 = client
        .post("/order", &[], Bytes::from_static(b"hello"))
        .await
        .expect("post after reconnect");
    assert_eq!(r2.status, 200);

    let stats = client.stats();
    assert!(stats.reconnects >= 1, "stats={stats:?}");
    assert!(stats.connects >= 2, "stats={stats:?}");

    let _ = tx.send(());
    client.shutdown().await;
}
