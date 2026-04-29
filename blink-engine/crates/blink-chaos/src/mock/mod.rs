//! Misbehaving mock servers used by the integration scenarios.
//!
//! Every mock binds `127.0.0.1:0` so tests can run in parallel-ish
//! without port collisions. Shutdown is cooperative via a `oneshot`
//! sender returned in the handle.

pub mod clob;
pub mod polygon_ws;
pub mod tls;
