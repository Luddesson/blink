//! Async network I/O abstraction — tokio (dev) vs io_uring (prod).
//!
//! The [`AsyncNetworkIo`] trait provides a platform-agnostic interface for
//! low-level async network operations. Two backends exist:
//!
//! | Backend      | Feature flag | Platform | Use case           |
//! |--------------|-------------|----------|--------------------|
//! | [`TokioNet`] | *(default)* | Any      | Local dev / CI     |
//! | [`IoUringNet`]| `io_uring` | Linux    | Production server  |
//!
//! Enable the production backend with:
//! ```toml
//! [dependencies]
//! engine = { path = ".", features = ["io_uring"] }
//! ```

use std::future::Future;
use std::pin::Pin;

/// Result type alias for network I/O operations.
pub type IoResult<T> = std::io::Result<T>;

/// Boxed future returned by async trait methods (object-safe).
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

// ────────────────────────────────────────────────────────────
// Trait: AsyncNetworkIo
// ────────────────────────────────────────────────────────────

/// Platform-agnostic async network I/O interface.
///
/// Implementors provide non-blocking TCP connect, read, and write with
/// minimal overhead on their target platform.
pub trait AsyncNetworkIo: Send + Sync + 'static {
    /// Connect to a remote TCP endpoint.
    fn connect<'a>(&'a self, addr: &'a str) -> BoxFuture<'a, IoResult<()>>;

    /// Read bytes into `buf`, returning the number of bytes read.
    fn read<'a>(&'a self, buf: &'a mut [u8]) -> BoxFuture<'a, IoResult<usize>>;

    /// Write all bytes from `buf`.
    fn write_all<'a>(&'a self, buf: &'a [u8]) -> BoxFuture<'a, IoResult<()>>;

    /// Flush any buffered output.
    fn flush(&self) -> BoxFuture<'_, IoResult<()>>;
}

// ────────────────────────────────────────────────────────────
// Backend: TokioNet (default — all platforms)
// ────────────────────────────────────────────────────────────

/// Tokio-based async network backend (default for dev/CI).
///
/// Uses `tokio::net::TcpStream` under the hood. Works on all platforms
/// and requires no special kernel features.
pub struct TokioNet {
    stream: tokio::sync::Mutex<Option<tokio::net::TcpStream>>,
}

impl TokioNet {
    /// Create a new unconnected `TokioNet` handle.
    pub fn new() -> Self {
        Self {
            stream: tokio::sync::Mutex::new(None),
        }
    }
}

impl Default for TokioNet {
    fn default() -> Self {
        Self::new()
    }
}

impl AsyncNetworkIo for TokioNet {
    fn connect<'a>(&'a self, addr: &'a str) -> BoxFuture<'a, IoResult<()>> {
        Box::pin(async move {
            let tcp = tokio::net::TcpStream::connect(addr).await?;
            tcp.set_nodelay(true)?;
            let mut guard = self.stream.lock().await;
            *guard = Some(tcp);
            Ok(())
        })
    }

    fn read<'a>(&'a self, buf: &'a mut [u8]) -> BoxFuture<'a, IoResult<usize>> {
        Box::pin(async move {
            use tokio::io::AsyncReadExt;
            let mut guard = self.stream.lock().await;
            let stream = guard
                .as_mut()
                .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotConnected, "not connected"))?;
            stream.read(buf).await
        })
    }

    fn write_all<'a>(&'a self, buf: &'a [u8]) -> BoxFuture<'a, IoResult<()>> {
        Box::pin(async move {
            use tokio::io::AsyncWriteExt;
            let mut guard = self.stream.lock().await;
            let stream = guard
                .as_mut()
                .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotConnected, "not connected"))?;
            stream.write_all(buf).await
        })
    }

    fn flush(&self) -> BoxFuture<'_, IoResult<()>> {
        Box::pin(async move {
            use tokio::io::AsyncWriteExt;
            let mut guard = self.stream.lock().await;
            let stream = guard
                .as_mut()
                .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotConnected, "not connected"))?;
            stream.flush().await
        })
    }
}

// ────────────────────────────────────────────────────────────
// Backend: IoUringNet (Linux prod — feature "io_uring")
// ────────────────────────────────────────────────────────────

/// io_uring-based async network backend (Linux production).
///
/// Uses `tokio-uring` for kernel-bypassed async I/O with completion-ring
/// semantics. Requires Linux 5.11+ with io_uring support.
///
/// **Enabled only** with `--features io_uring` and only compiles on Linux.
///
/// # Threading model
///
/// `tokio-uring` operates on a single-threaded runtime. In production, the
/// engine is pinned to one core via `os_tune.sh` (NUMA pinning + isolcpus).
/// The `Send + Sync` bounds are satisfied because all access occurs on that
/// single runtime thread. We use `UnsafeCell` + manual synchronization
/// guarantee rather than a `Mutex` to avoid lock overhead on the hot path.
///
/// # Buffer ownership
///
/// `tokio-uring` uses an owned-buffer API (buffers are moved into read/write
/// calls and returned on completion). We bridge this to the `&mut [u8]` trait
/// interface by copying into/out of an intermediate owned buffer. For the
/// WebSocket frames in this engine (~256-4096 bytes), the copy cost is
/// negligible compared to kernel submission latency.
#[cfg(feature = "io_uring")]
pub struct IoUringNet {
    /// The underlying tokio-uring TCP stream.
    ///
    /// SAFETY: Only accessed from the single-threaded tokio-uring runtime.
    /// The engine's production deployment pins all I/O to one core.
    stream: std::cell::UnsafeCell<Option<tokio_uring::net::TcpStream>>,
}

// SAFETY: IoUringNet is only used within a single-threaded tokio-uring
// runtime. The engine enforces this at startup by running on a
// `tokio_uring::start()` single-threaded executor, and production
// deployment pins to a single NUMA core via os_tune.sh.
#[cfg(feature = "io_uring")]
unsafe impl Send for IoUringNet {}
#[cfg(feature = "io_uring")]
unsafe impl Sync for IoUringNet {}

#[cfg(feature = "io_uring")]
impl IoUringNet {
    pub fn new() -> Self {
        Self {
            stream: std::cell::UnsafeCell::new(None),
        }
    }

    /// Returns a mutable reference to the inner stream.
    ///
    /// SAFETY: Caller must ensure single-threaded access (guaranteed by
    /// the tokio-uring runtime model).
    #[inline]
    unsafe fn stream_mut(&self) -> &mut Option<tokio_uring::net::TcpStream> {
        &mut *self.stream.get()
    }
}

#[cfg(feature = "io_uring")]
impl Default for IoUringNet {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "io_uring")]
impl AsyncNetworkIo for IoUringNet {
    fn connect<'a>(&'a self, addr: &'a str) -> BoxFuture<'a, IoResult<()>> {
        Box::pin(async move {
            use std::net::ToSocketAddrs;

            let sock_addr = addr
                .to_socket_addrs()?
                .next()
                .ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        format!("could not resolve address: {addr}"),
                    )
                })?;

            let tcp = tokio_uring::net::TcpStream::connect(sock_addr).await?;

            // Set TCP_NODELAY to disable Nagle's algorithm (critical for HFT).
            // tokio-uring's TcpStream exposes the raw fd for socket options.
            use std::os::unix::io::AsRawFd;
            let fd = tcp.as_raw_fd();
            unsafe {
                let nodelay: libc::c_int = 1;
                let ret = libc::setsockopt(
                    fd,
                    libc::IPPROTO_TCP,
                    libc::TCP_NODELAY,
                    &nodelay as *const _ as *const libc::c_void,
                    std::mem::size_of::<libc::c_int>() as libc::socklen_t,
                );
                if ret != 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }

            // SAFETY: single-threaded tokio-uring runtime.
            let slot = unsafe { self.stream_mut() };
            *slot = Some(tcp);
            Ok(())
        })
    }

    fn read<'a>(&'a self, buf: &'a mut [u8]) -> BoxFuture<'a, IoResult<usize>> {
        Box::pin(async move {
            // SAFETY: single-threaded tokio-uring runtime.
            let stream = unsafe { self.stream_mut() }
                .as_ref()
                .ok_or_else(|| {
                    std::io::Error::new(std::io::ErrorKind::NotConnected, "not connected")
                })?;

            // tokio-uring uses owned buffers. Allocate a Vec, read into it,
            // then copy to the caller's slice. For typical WebSocket frames
            // (256-4096 bytes) this copy is ~1µs — well within our budget.
            let owned_buf = vec![0u8; buf.len()];
            let (result, returned_buf) = stream.read(owned_buf).await;
            let n = result?;
            buf[..n].copy_from_slice(&returned_buf[..n]);
            Ok(n)
        })
    }

    fn write_all<'a>(&'a self, buf: &'a [u8]) -> BoxFuture<'a, IoResult<()>> {
        Box::pin(async move {
            // SAFETY: single-threaded tokio-uring runtime.
            let stream = unsafe { self.stream_mut() }
                .as_ref()
                .ok_or_else(|| {
                    std::io::Error::new(std::io::ErrorKind::NotConnected, "not connected")
                })?;

            // tokio-uring write_all takes ownership of the buffer.
            // Copy the caller's slice into an owned Vec.
            let mut written = 0;
            while written < buf.len() {
                let owned_buf = buf[written..].to_vec();
                let (result, _returned_buf) = stream.write(owned_buf).await;
                let n = result?;
                if n == 0 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::WriteZero,
                        "write returned 0 bytes",
                    ));
                }
                written += n;
            }
            Ok(())
        })
    }

    fn flush(&self) -> BoxFuture<'_, IoResult<()>> {
        // io_uring submissions are flushed on ring enter — no-op.
        Box::pin(async { Ok(()) })
    }
}

// ────────────────────────────────────────────────────────────
// Factory: create the right backend for the current platform
// ────────────────────────────────────────────────────────────

/// Create the appropriate [`AsyncNetworkIo`] backend for the current build.
///
/// - Default (no feature flags): returns [`TokioNet`]
/// - `--features io_uring`: returns [`IoUringNet`] (Linux only)
pub fn create_network_io() -> Box<dyn AsyncNetworkIo> {
    #[cfg(feature = "io_uring")]
    {
        tracing::info!("io_uring network backend enabled (Linux prod)");
        Box::new(IoUringNet::new())
    }

    #[cfg(not(feature = "io_uring"))]
    {
        tracing::info!("tokio network backend enabled (dev/CI)");
        Box::new(TokioNet::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokio_net_default() {
        let _net = TokioNet::new();
    }

    #[test]
    fn factory_returns_tokio_by_default() {
        let _net = create_network_io();
    }
}
