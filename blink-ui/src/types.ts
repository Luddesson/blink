// ─── Engine mode ─────────────────────────────────────────────────────────────
export type EngineMode = 'paper' | 'live' | 'readonly'
export type StrategyMode = 'mirror' | 'conservative' | 'aggressive'

export interface StrategySwitchRecord {
  seq: number
  switched_at_ms: number
  from: StrategyMode
  to: StrategyMode
  reason?: string | null
  source: string
}

export interface StrategyStatus {
  current_mode: StrategyMode
  switch_seq: number
  last_switched_at_ms: number
  cooldown_secs: number
  runtime_switch_enabled: boolean
  live_switch_allowed: boolean
  require_reason: boolean
  cooldown_remaining_ms?: number
  profile?: {
    min_notional_multiplier: number
    sizing_multiplier: number
    price_band_lo_adjust: number
    price_band_hi_adjust: number
  }
  history?: StrategySwitchRecord[]
}

export interface ModeResponse {
  mode: EngineMode
  live_trading_env: boolean
  paper_active: boolean
  live_active: boolean
  strategy?: StrategyStatus
}

// ─── WebSocket snapshot ──────────────────────────────────────────────────────
export interface WsSnapshot {
  type: 'snapshot'
  timestamp_ms: number
  /** Monotonic sequence number — detect gaps or out-of-order delivery */
  snapshot_seq?: number
  /** Engine uptime in seconds (authoritative, from server clock) */
  engine_uptime_secs?: number
  /** How stale the portfolio cache is, in milliseconds */
  portfolio_age_ms?: number
  ws_connected: boolean
  trading_paused: boolean
  messages_total: number
  portfolio?: PortfolioSummary
  risk?: RiskSummary
  recent_activity?: ActivityEntry[]
  vol_bps?: number  // global rolling volatility (CoV × 10000)
  /** Live order book summaries — keyed by token_id (from 6A) */
  order_books?: Record<string, OrderBookSummary>
  strategy?: StrategyStatus
}

// ─── Portfolio ───────────────────────────────────────────────────────────────
export interface PortfolioSummary {
  cash_usdc: number
  nav_usdc: number
  blink_cash_usdc?: number
  blink_nav_usdc?: number
  wallet_nav_usdc?: number | null
  invested_usdc: number
  unrealized_pnl_usdc: number
  realized_pnl_usdc: number
  fees_paid_usdc: number
  cash_source?: string
  balance_source?: string
  exchange_position_value_usdc?: number
  external_position_value_usdc?: number
  wallet_position_value_usdc?: number | null
  wallet_position_initial_value_usdc?: number | null
  wallet_open_pnl_usdc?: number | null
  wallet_unrealized_pnl_usdc?: number | null
  wallet_pnl_source?: string
  pnl_source?: string
  exchange_positions_count?: number
  exchange_positions_preview?: WalletPositionPreview[]
  wallet_positions_count?: number
  reality_status?: 'matched' | 'mismatch' | 'unverified'
  reality_issues?: string[]
  truth_checked_at_ms?: number | null
  exchange_positions_verified?: boolean
  onchain_cash_verified?: boolean
  wallet_truth_verified?: boolean
  blink_wallet_truth_last_sync_ms?: number | null
  blink_wallet_truth_sync_age_ms?: number | null
  external_only_positions_count?: number
  local_only_positions_count?: number
  local_open_positions_count?: number
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
  event_start_time?: number  // Unix timestamp — game/event kickoff
  event_end_time?: number    // Unix timestamp — market resolution deadline
  secs_to_event?: number     // seconds until market resolution (pre-computed by engine, can be negative)
}

export interface WalletPositionPreview {
  title?: string | null
  outcome?: string | null
  size?: number | string | null
  current_value_usdc?: number | string | null
  cash_pnl_usdc?: number | string | null
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
  event_start_time?: number  // Unix timestamp — game/event kickoff
  event_end_time?: number    // Unix timestamp — market resolution deadline
  signal_source?: string
  analysis_id?: string
}

export interface HistoryResponse {
  trades: ClosedTrade[]
  total: number
  page: number
  per_page: number
  total_pages: number
}

export interface LiveExecution {
  transaction_hash?: string | null
  token_id: string
  condition_id?: string | null
  market_title?: string | null
  market_outcome?: string | null
  side: string
  price: number
  shares: number
  usdc_size: number
  timestamp: number
  traded_at: string
  execution_type: string
  source: string
}

export interface LiveExecutionsResponse {
  executions: LiveExecution[]
  total: number
  page: number
  per_page: number
  total_pages: number
  source?: string
  range?: string
  reality_status?: 'matched' | 'mismatch' | 'unverified'
  truth_checked_at_ms?: number | null
  reality_issues?: string[]
}

// ─── Risk ────────────────────────────────────────────────────────────────────
export interface RiskSummary {
  trading_enabled: boolean
  circuit_breaker_tripped?: boolean
  circuit_breaker?: boolean
  circuit_breaker_reason?: string
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
  accounting_source?: string
  balance_source?: string
  cash_source?: string
  confirmed_only?: boolean
  queued_orders_affect_nav?: boolean
  cash_usdc?: number | null
  nav_usdc?: number | null
  blink_cash_usdc?: number
  blink_nav_usdc?: number
  wallet_nav_usdc?: number | null
  invested_usdc?: number
  unrealized_pnl_usdc?: number
  realized_pnl_usdc?: number
  fees_paid_usdc?: number
  open_positions_count?: number
  wallet_positions_count?: number
  exchange_position_value_usdc?: number
  external_position_value_usdc?: number
  wallet_position_value_usdc?: number | null
  wallet_position_initial_value_usdc?: number | null
  wallet_open_pnl_usdc?: number | null
  wallet_unrealized_pnl_usdc?: number | null
  wallet_pnl_source?: string
  pnl_source?: string
  exchange_positions_count?: number
  exchange_positions_preview?: WalletPositionPreview[]
  reality_status?: 'matched' | 'mismatch' | 'unverified'
  reality_issues?: string[]
  truth_checked_at_ms?: number | null
  exchange_positions_verified?: boolean
  onchain_cash_verified?: boolean
  wallet_truth_verified?: boolean
  blink_wallet_truth_last_sync_ms?: number | null
  blink_wallet_truth_sync_age_ms?: number | null
  external_only_positions_count?: number
  local_only_positions_count?: number
  local_open_positions_count?: number
  open_positions?: Position[]
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
  uptime_secs?: number
  strategy?: StrategyStatus
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

// ─── Bullpen ─────────────────────────────────────────────────────────────────
export interface BullpenHealthResponse {
  enabled: boolean
  authenticated?: boolean
  consecutive_failures?: number
  total_calls?: number
  avg_latency_ms?: number
  last_error?: string | null
  status?: 'disabled' | 'unwired' | string
  source?: string
  truth_checked_at_ms?: number
}

export interface BullpenDiscoveryResponse {
  enabled: boolean
  total_markets?: number
  scan_count?: number
  markets?: BullpenDiscoveredMarket[]
  status?: 'disabled' | 'unwired' | string
  source?: string
  truth_checked_at_ms?: number
}

export interface BullpenDiscoveredMarket {
  token_id: string
  title?: string | null
  lenses: string[]
  viability_score: number
  conviction_boost: number
  smart_money_interest: boolean
  seen_count: number
}

export interface BullpenConvergenceResponse {
  enabled: boolean
  active_signals?: number
  tracked_markets?: number
  signals?: BullpenConvergenceSignal[]
  status?: 'disabled' | 'unwired' | string
  source?: string
  truth_checked_at_ms?: number
}

export interface BullpenConvergenceSignal {
  market_title?: string
  wallet_count: number
  convergence_score: number
  net_direction: string
  total_usd: number
}

// ─── Metrics ─────────────────────────────────────────────────────────────────
export interface MetricsResponse {
  available: boolean
  signals_rejected_last_60s?: number
  rejection_by_reason?: Record<string, number>
  uptime_secs?: number
}

// ─── Order Book ──────────────────────────────────────────────────────────────
export interface OrderBookResponse {
  token_id: string
  market_title?: string | null
  bids: [number, number][]  // [price, size]
  asks: [number, number][]
  best_bid: number | null
  best_ask: number | null
  spread_bps: number | null
}

/** Lightweight order book summary sent in the WS snapshot (6A) */
export interface OrderBookSummary {
  bid_depth: number
  ask_depth: number
  best_bid: number | null
  best_ask: number | null
  spread_bps: number
  imbalance: number  // -1 (all asks) to +1 (all bids)
}

/** Response from /api/pnl-attribution (6B) */
export interface PnlAttributionResponse {
  available: boolean
  total_trades?: number
  by_reason: Record<string, number>
  by_category: Record<string, number>
  by_side: Record<string, number>
}

export interface OrderBooksResponse {
  orderbooks: OrderBookResponse[]
}

// ─── Twin ────────────────────────────────────────────────────────────────────
export interface TwinSnapshot {
  generation: number
  extra_latency_ms: number
  slippage_penalty_bps: number
  drift_multiplier: number
  nav: number
  realized_pnl: number
  unrealized_pnl: number
  filled_orders: number
  aborted_orders: number
  open_positions: number
  closed_trades: number
  win_rate_pct: number
  nav_return_pct: number
  max_drawdown_pct: number
}

export type ProjectInventoryStatus =
  | 'active-runtime'
  | 'active-ops'
  | 'compiled-not-wired'
  | 'archived-or-legacy'
  | 'unknown-needs-review'

export type ProjectInventoryEvidence = {
  path: string
  line?: number
  note: string
}

export type ProjectInventoryItem = {
  id: string
  area: string
  name: string
  status: ProjectInventoryStatus
  confidence: string
  recommendation: string
  evidence: ProjectInventoryEvidence[]
}

export type ProjectInventorySummary = {
  totalItems: number
  byStatus: Record<string, number>
  byArea: Record<string, number>
}

export type ProjectInventoryResponse = {
  available: boolean
  schemaVersion?: number
  generatedAt?: string
  summary?: ProjectInventorySummary
  items?: ProjectInventoryItem[]
  error?: string
  generate_command?: string
  paths_checked?: string[]
}
