// ─── Engine mode ─────────────────────────────────────────────────────────────
export type EngineMode = 'paper' | 'live' | 'readonly'

export interface ModeResponse {
  mode: EngineMode
  live_trading_env: boolean
  paper_active: boolean
  live_active: boolean
}

// ─── WebSocket snapshot ──────────────────────────────────────────────────────
export interface WsSnapshot {
  type: 'snapshot'
  timestamp_ms: number
  ws_connected: boolean
  trading_paused: boolean
  messages_total: number
  portfolio?: PortfolioSummary
  risk?: RiskSummary
  recent_activity?: ActivityEntry[]
}

// ─── Portfolio ───────────────────────────────────────────────────────────────
export interface PortfolioSummary {
  cash_usdc: number
  nav_usdc: number
  invested_usdc: number
  unrealized_pnl_usdc: number
  realized_pnl_usdc: number
  fees_paid_usdc: number
  // WS snapshot sends open_positions as Position[]; REST /api/portfolio also sends array
  open_positions: Position[]
  closed_trades_count: number
  total_signals: number
  filled_orders: number
  skipped_orders: number
  aborted_orders: number
  fill_rate_pct: number
  reject_rate_pct?: number
  equity_curve: number[]
  equity_timestamps: number[]
  win_rate_pct: number
  uptime_secs: number
  avg_slippage_bps?: number
}

export interface Position {
  id: number
  token_id: string
  market_title?: string
  market_outcome?: string
  side: string
  entry_price: number
  shares: number
  usdc_spent: number
  current_price: number
  unrealized_pnl: number
  unrealized_pnl_pct: number
  opened_at?: string
  opened_age_secs: number
}

// /api/portfolio returns same shape as PortfolioSummary; alias for clarity
export type FullPortfolio = PortfolioSummary

export interface ClosedTrade {
  token_id: string
  market_title?: string
  side: string
  entry_price: number
  exit_price: number
  shares: number
  realized_pnl: number
  fees_paid_usdc: number
  reason: string
  opened_at: string
  closed_at: string
  duration_secs: number
  slippage_bps: number
}

export interface HistoryResponse {
  trades: ClosedTrade[]
  total: number
  page: number
  per_page: number
  total_pages: number
}

// ─── Risk ────────────────────────────────────────────────────────────────────
export interface RiskSummary {
  trading_enabled: boolean
  circuit_breaker_tripped?: boolean
  circuit_breaker?: boolean
  daily_pnl: number
  max_daily_loss_pct?: number
  max_concurrent_positions?: number
  max_single_order_usdc?: number
  max_orders_per_second?: number
  var_threshold_pct?: number
}

// ─── Live portfolio ──────────────────────────────────────────────────────────
export interface LivePortfolio {
  mode: 'live'
  pending_orders: number
  confirmed_fills: number
  no_fills: number
  stale_orders: number
  confirmation_rate_pct?: number
  daily_pnl_usdc: number
  max_daily_loss_pct: number
  circuit_breaker_tripped: boolean
  trading_enabled: boolean
  heartbeat_ok: number
  heartbeat_fail: number
  trigger_count: number
  uptime_secs: number
}

// ─── Failsafe ────────────────────────────────────────────────────────────────
export interface FailsafeSnapshot {
  available: boolean
  trigger_count: number
  check_count: number
  max_observed_drift_bps: number
  confirmed_fills: number
  no_fills: number
  stale_orders: number
  confirmation_rate_pct?: number
  heartbeat_ok_count: number
  heartbeat_fail_count: number
}

// ─── Activity ────────────────────────────────────────────────────────────────
export interface ActivityEntry {
  timestamp: string
  kind: string
  message: string
}

// ─── Latency ─────────────────────────────────────────────────────────────────
export interface LatencyResponse {
  signal_age: {
    min_us?: number
    avg_us?: number
    max_us?: number
    p50_us?: number
    p95_us?: number
    p99_us?: number
    p999_us?: number
    count?: number
    histogram?: number[]
  }
  ws_msg_per_sec: number
}

// ─── Status ──────────────────────────────────────────────────────────────────
export interface StatusResponse {
  timestamp_ms: number
  ws_connected: boolean
  trading_paused: boolean
  messages_total: number
  subscriptions: string[]
  risk_status: 'OK' | 'CIRCUIT_BREAKER' | 'KILL_SWITCH_OFF' | 'N/A'
}

// ─── Fill Window ─────────────────────────────────────────────────────────────
export interface FillWindowResponse {
  available: boolean
  reason?: string
  token_id?: string
  side?: string
  entry_price?: number
  current_price?: number
  drift_pct?: number
  elapsed_secs?: number
  countdown_secs?: number
}

// ─── Metrics ─────────────────────────────────────────────────────────────────
export interface MetricsResponse {
  available: boolean
  signals_rejected_last_60s?: number
  rejection_by_reason?: Record<string, number>
  uptime_secs?: number
}
