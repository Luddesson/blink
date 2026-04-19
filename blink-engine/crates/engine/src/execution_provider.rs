use std::sync::{Arc, Mutex};

/// ExecutionProvider trait and a local stub implementation.
///
/// This is a lightweight scaffold used for wiring custodial provider integrations
/// without touching any secrets. Implement real providers (Fireblocks, BitGo,
/// Exchange API) behind this trait in future work.

pub trait ExecutionProvider: Send + Sync {
    /// Human-readable provider name.
    fn name(&self) -> String;

    /// Return available USDC balance (simulated for stubs).
    fn get_balance_usdc(&self) -> f64;

    /// Send a sell for `shares` of `token_id`. Returns USDC received on success.
    fn send_sell(&self, token_id: &str, shares: f64) -> Result<f64, String>;

    /// Settle USDC to the configured cash account (no-op for stub).
    fn settle_usdc(&self, amount: f64) -> Result<(), String>;
}

/// Simple in-process stub provider for local testing. Thread-safe via Mutex.
pub struct LocalStub {
    balance: Mutex<f64>,
    pub name: String,
}

impl LocalStub {
    pub fn new(starting_balance: f64) -> Self {
        Self {
            balance: Mutex::new(starting_balance),
            name: "local_stub".to_string(),
        }
    }
}

impl ExecutionProvider for LocalStub {
    fn name(&self) -> String {
        self.name.clone()
    }

    fn get_balance_usdc(&self) -> f64 {
        *self.balance.lock().unwrap()
    }

    fn send_sell(&self, _token_id: &str, shares: f64) -> Result<f64, String> {
        // For the stub, interpret `shares` as USDC received directly.
        // In real providers this would submit an on-chain or exchange order
        // and return the settled USDC amount after fees.
        let mut bal = self.balance.lock().unwrap();
        let received = shares; // identity mapping for stub
        *bal += received;
        Ok(received)
    }

    fn settle_usdc(&self, _amount: f64) -> Result<(), String> {
        // No-op for local stub.
        Ok(())
    }
}

/// Create a provider based on environment variables. Returns `Some(Arc<dyn ExecutionProvider>)`
/// when a provider is configured (currently supports "local_stub"), otherwise `None`.
pub fn create_provider_from_env() -> Option<Arc<dyn ExecutionProvider>> {
    let prov = std::env::var("CUSTODIAL_PROVIDER").unwrap_or_else(|_| "none".to_string());
    match prov.as_str() {
        "local_stub" | "stub" => Some(Arc::new(LocalStub::new(
            std::env::var("STARTING_USDC")
                .ok()
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(100.0),
        ))),
        _ => None,
    }
}
