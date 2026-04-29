//! `MockPolygonWs` — tokio-tungstenite-based WS server with injectable
//! misbehaviour. Used by scenarios that exercise ingress reconnect
//! logic (CLOB WS / Polygon logs WS).
//!
//! Behaviour is controlled by [`MockWsBehaviour`], which is held behind
//! an `Arc<Mutex<_>>` so tests can reconfigure mid-run.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::Message;

/// Controls how the mock behaves across accepted connections.
#[derive(Debug, Clone)]
pub struct MockWsBehaviour {
    /// Frames to send (verbatim) to each client after accept.
    pub frames: Vec<String>,
    /// If `Some(n)`, close the socket after sending the first `n`
    /// frames — used to simulate a mid-stream server drop. `None`
    /// means serve all frames then keep the stream open.
    pub drop_after: Option<usize>,
    /// Stall `stall_every_nth_frame` — every Nth frame is delayed by
    /// `stall_ms`. `0` means no stalls.
    pub stall_every_nth_frame: u32,
    /// Duration to stall for when `stall_every_nth_frame` hits.
    pub stall_ms: u32,
    /// If `true`, refuse the TCP `accept` outright. Flip off mid-test
    /// to simulate a delayed recovery.
    pub refuse_connects: bool,
    /// Inter-frame delay; 0 = as fast as possible.
    pub inter_frame_ms: u32,
    /// How many accepts we've served — the mock resets its frame
    /// cursor per accept.
    pub accepts_served: u32,
}

impl Default for MockWsBehaviour {
    fn default() -> Self {
        Self {
            frames: Vec::new(),
            drop_after: None,
            stall_every_nth_frame: 0,
            stall_ms: 0,
            refuse_connects: false,
            inter_frame_ms: 0,
            accepts_served: 0,
        }
    }
}

/// Handle to a running `MockPolygonWs`.
pub struct MockPolygonWs {
    /// `ws://127.0.0.1:<port>` URL the victim source should connect to.
    pub url: String,
    /// Bound socket address — exposed for assertions that want the
    /// port directly.
    pub addr: SocketAddr,
    /// Shared behaviour knob. Callers update via `server.behaviour.lock()`.
    pub behaviour: Arc<Mutex<MockWsBehaviour>>,
    /// Total accepts observed (including refused ones).
    pub accepts: Arc<AtomicU64>,
    shutdown: Option<oneshot::Sender<()>>,
}

impl MockPolygonWs {
    /// Bind `127.0.0.1:0` and start serving.
    pub async fn spawn(initial: MockWsBehaviour) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("mock-polygon-ws: bind");
        let addr = listener.local_addr().expect("local_addr");
        let url = format!("ws://127.0.0.1:{}", addr.port());

        let behaviour = Arc::new(Mutex::new(initial));
        let accepts = Arc::new(AtomicU64::new(0));
        let (tx, mut rx) = oneshot::channel::<()>();

        let beh_cl = behaviour.clone();
        let acc_cl = accepts.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut rx => return,
                    accept = listener.accept() => {
                        let (tcp, _peer) = match accept {
                            Ok(v) => v,
                            Err(_) => continue,
                        };
                        acc_cl.fetch_add(1, Ordering::Relaxed);
                        if beh_cl.lock().refuse_connects {
                            drop(tcp);
                            continue;
                        }
                        let beh = beh_cl.clone();
                        tokio::spawn(serve_conn(tcp, beh));
                    }
                }
            }
        });

        Self { url, addr, behaviour, accepts, shutdown: Some(tx) }
    }

    /// Cooperative shutdown.
    pub fn shutdown(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

impl Drop for MockPolygonWs {
    fn drop(&mut self) {
        self.shutdown();
    }
}

async fn serve_conn(tcp: tokio::net::TcpStream, beh: Arc<Mutex<MockWsBehaviour>>) {
    let ws = match tokio_tungstenite::accept_async(tcp).await {
        Ok(w) => w,
        Err(_) => return,
    };
    let (frames, drop_after, stall_every, stall_ms, inter_ms) = {
        let mut g = beh.lock();
        g.accepts_served = g.accepts_served.saturating_add(1);
        (
            g.frames.clone(),
            g.drop_after,
            g.stall_every_nth_frame,
            g.stall_ms,
            g.inter_frame_ms,
        )
    };
    let (mut sink, mut stream) = ws.split();

    // Swallow incoming messages; mocks don't care about subscribe frames.
    let reader = tokio::spawn(async move {
        while let Some(next) = stream.next().await {
            if next.is_err() {
                break;
            }
        }
    });

    let mut sent = 0usize;
    for (i, frame) in frames.into_iter().enumerate() {
        if inter_ms > 0 {
            tokio::time::sleep(Duration::from_millis(inter_ms as u64)).await;
        }
        if stall_every > 0 && (i as u32 + 1) % stall_every == 0 && stall_ms > 0 {
            tokio::time::sleep(Duration::from_millis(stall_ms as u64)).await;
        }
        if sink.send(Message::Text(frame.into())).await.is_err() {
            return;
        }
        sent += 1;
        if let Some(n) = drop_after {
            if sent >= n {
                let _ = sink.close().await;
                reader.abort();
                return;
            }
        }
    }
    // No drop-after — hold the stream open until the client goes away.
    let _ = reader.await;
}
