use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::sleep;
use tracing::{info, warn, error};

use crate::activity_log::{ActivityLog, EntryKind, push as log_push};
use crate::config::Config;
use crate::mev_router::MevRouter;
use crate::order_book::OrderBookStore;
use crate::order_executor::OrderExecutor;
use crate::order_signer::{sign_order_with_vault, OrderParams};
use crate::paper_portfolio::{DRIFT_THRESHOLD, PaperPortfolio, STARTING_BALANCE_USDC};
use crate::risk_manager::{RiskConfig, RiskManager};
use crate::types::{OrderSide, RN1Signal, TimeInForce};

pub struct LiveEngine {
    pub portfolio: Arc<Mutex<PaperPortfolio>>,
    book_store: Arc<OrderBookStore>,
    activity: Option<ActivityLog>,
    executor: OrderExecutor,
    vault: Option<Arc<tee_vault::VaultHandle>>,
    funder_addr: String,
    risk: std::sync::Mutex<RiskManager>,
    mev_router: Option<std::sync::Mutex<MevRouter>>,
}

impl LiveEngine {
    pub fn new(config: Arc<Config>, book_store: Arc<OrderBookStore>, activity: Option<ActivityLog>) -> Self {
        let executor = OrderExecutor::from_config(&config);

        // Initialize the TEE vault for key isolation.
        let vault = if config.live_trading && !config.signer_private_key.is_empty() {
            match tee_vault::VaultHandle::spawn(&config.signer_private_key) {
                Ok(handle) => {
                    info!(
                        address = %handle.signer_address(),
                        "TEE vault initialized — private key isolated in vault task"
                    );
                    Some(Arc::new(handle))
                }
                Err(e) => {
                    error!(error = %e, "Failed to initialize TEE vault — live signing disabled");
                    None
                }
            }
        } else {
            None
        };

        let funder_addr = config.funder_address.clone();
        let risk = RiskManager::new(RiskConfig::from_env());

        if let Some(ref log) = activity {
            log_push(
                log,
                EntryKind::Engine,
                format!(
                    "LiveEngine started — live={} vault={} dry_run={}",
                    config.live_trading, vault.is_some(), !config.live_trading
                ),
            );
        }

        // Initialize MEV router from env if MEV_ROUTER is set.
        let mev = if std::env::var("MEV_ROUTER").is_ok() {
            info!("MEV router enabled");
            Some(std::sync::Mutex::new(MevRouter::from_env()))
        } else {
            None
        };

        Self {
            portfolio: Arc::new(Mutex::new(PaperPortfolio::new())),
            book_store,
            activity,
            executor,
            vault,
            funder_addr,
            risk: std::sync::Mutex::new(risk),
            mev_router: mev,
        }
    }

    /// Flush all open positions from the internal order cache.
    ///
    /// Called when a game-start signal is detected — the CLOB clears all
    /// outstanding orders, so our local state must be flushed too.
    pub async fn flush_order_cache(&self, token_id: &str) {
        let mut p = self.portfolio.lock().await;
        let before = p.positions.len();
        p.positions.retain(|pos| pos.token_id != token_id);
        let after = p.positions.len();
        let flushed = before - after;

        if flushed > 0 {
            warn!(
                token_id,
                flushed,
                "🧹 Pre-game order wipe: flushed {flushed} positions for {token_id}"
            );
            if let Some(ref log) = self.activity {
                log_push(
                    log,
                    EntryKind::Warn,
                    format!("GAME START: flushed {flushed} positions for {token_id}"),
                );
            }
        }
    }

    pub async fn handle_signal(&self, signal: RN1Signal) {
        // 1. Calculate entry_price, rn1_shares, rn1_notional_usd
        let entry_price = signal.price as f64 / 1_000.0;
        let rn1_shares = signal.size as f64 / 1_000.0;
        let rn1_notional_usd = rn1_shares * entry_price;

        // 2. Size the order — brief lock on portfolio
        let (size_usdc, current_nav, open_positions) = {
            let mut p = self.portfolio.lock().await;
            p.total_signals += 1;
            let size = p.calculate_size_usdc(rn1_notional_usd);
            let nav = p.nav();
            let open = p.positions.len();
            (size, nav, open)
        };

        // 3. Risk check — BEFORE doing anything real
        let size_usdc = match size_usdc {
            Some(s) => s,
            None => {
                // Skip like PaperEngine
                return;
            }
        };

        if let Err(violation) = self.risk.lock().unwrap().check_pre_order(
            size_usdc,
            open_positions,
            current_nav,
            STARTING_BALANCE_USDC,
        ) {
            warn!("🛑 Risk check blocked order: {violation}");
            if let Some(ref log) = self.activity {
                log_push(log, EntryKind::Warn, format!("BLOCKED: {violation}"));
            }
            return;
        }

        // 4. Fill window check — same as PaperEngine
        let filled = self
            .check_fill_window(&signal.token_id, entry_price, signal.side)
            .await;
        if !filled {
            // Abort like PaperEngine
            return;
        }

        // 5. Build and sign (or dry-run)
        let params = OrderParams {
            token_id: signal.token_id.clone(),
            side: signal.side,
            price: signal.price,
            size: size_usdc,
            maker: self.funder_addr.clone(),
        };

        let _order_result = if self.executor.dry_run || self.vault.is_none() {
            info!("DRY-RUN: would sign & submit order");
            if let Some(ref log) = self.activity {
                log_push(
                    log,
                    EntryKind::Fill,
                    format!(
                        "DRY-RUN {} @{:.3} ${:.2}",
                        signal.side, entry_price, size_usdc
                    ),
                );
            }
            None // skip actual submission
        } else {
            let vault = self.vault.as_ref().unwrap();
            match sign_order_with_vault(vault.as_ref(), &params) {
                Ok(signed) => {
                    match self.executor.submit_order(&signed, TimeInForce::Gtc).await {
                        Ok(resp) => {
                            if resp.success {
                                info!(order_id = ?resp.order_id, "✅ LIVE order submitted");
                                if let Some(ref log) = self.activity {
                                    log_push(
                                        log,
                                        EntryKind::Fill,
                                        format!(
                                            "LIVE SUBMITTED {} @{:.3} ${:.2} id={:?}",
                                            signal.side, entry_price, size_usdc, resp.order_id
                                        ),
                                    );
                                }
                            } else {
                                error!(error = ?resp.error_msg, "❌ LIVE order rejected");
                            }
                            Some(resp)
                        }
                        Err(e) => {
                            error!(error = %e, "❌ LIVE submit failed");
                            None
                        }
                    }
                }
                Err(e) => {
                    error!(error = %e, "❌ EIP-712 signing failed");
                    None
                }
            }
        };

        // 6. Record virtual fill (always — for portfolio tracking)
        {
            let mut p = self.portfolio.lock().await;
            p.open_position(
                signal.token_id.clone(),
                signal.side,
                entry_price,
                size_usdc,
                signal.order_id.clone(),
            );
        }

        // Update risk manager
        self.risk.lock().unwrap().record_fill(size_usdc);
    }

    async fn check_fill_window(&self, token_id: &str, entry_price: f64, _side: OrderSide) -> bool {
        for check in 0_u8..6 {
            sleep(Duration::from_millis(500)).await;
            if let Some(current) = self.get_market_price(token_id) {
                let drift = (current - entry_price).abs() / entry_price;
                if drift > DRIFT_THRESHOLD {
                    warn!(
                        check,
                        "🚨 Fill window abort: price drifted {:.2}%",
                        drift * 100.0
                    );
                    return false;
                }
            }
        }
        true
    }

    fn get_market_price(&self, token_id: &str) -> Option<f64> {
        self.book_store
            .get_mid_price(token_id)
            .map(|p| p as f64 / 1_000.0)
    }
}
