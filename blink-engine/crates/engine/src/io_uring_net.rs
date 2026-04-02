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
/// Uses `tokio-uring` for kernel-bypassed async I/O with zero-copy
/// submission/completion rings. Requires Linux 5.11+ with io_uring support.
///
/// **Enabled only** with `--features io_uring` and only compiles on Linux.
#[cfg(feature = "io_uring")]
pub struct IoUringNet {
    // TODO(aura-1): Integrate tokio-uring TcpStream when deploying to Linux.
    //
    // Implementation plan:
    //   1. tokio_uring::net::TcpStream for connection management
    //   2. Fixed-buffer pool (registered with the ring) for zero-copy reads
    //   3. SQ polling mode (IORING_SETUP_SQPOLL) for submission without syscalls
    //   4. Buffer groups (IORING_OP_PROVIDE_BUFFERS) for recv multishot
    //
    // Dependencies (add to Cargo.toml when enabling):
    //   tokio-uring = "0.5"
    _placeholder: (),
}

#[cfg(feature = "io_uring")]
impl IoUringNet {
    pub fn new() -> Self {
        Self { _placeholder: () }
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
    fn connect<'a>(&'a self, _addr: &'a str) -> BoxFuture<'a, IoResult<()>> {
        // TODO(aura-1): Implement via tokio_uring::net::TcpStream::connect
        Box::pin(async { todo!("io_uring connect — implement for Linux prod") })
    }

    fn read<'a>(&'a self, _buf: &'a mut [u8]) -> BoxFuture<'a, IoResult<usize>> {
        // TODO(aura-1): Implement via registered buffer read
        Box::pin(async { todo!("io_uring read — implement for Linux prod") })
    }

    fn write_all<'a>(&'a self, _buf: &'a [u8]) -> BoxFuture<'a, IoResult<()>> {
        // TODO(aura-1): Implement via registered buffer write
        Box::pin(async { todo!("io_uring write_all — implement for Linux prod") })
    }

    fn flush(&self) -> BoxFuture<'_, IoResult<()>> {
        // io_uring submissions are flushed on submit — no-op
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
