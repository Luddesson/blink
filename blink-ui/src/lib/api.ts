import type {
  BullpenConvergenceResponse,
  BullpenDiscoveryResponse,
  BullpenHealthResponse,
  FailsafeSnapshot,
  FillWindowResponse,
  FullPortfolio,
  HistoryResponse,
  LatencyResponse,
  LivePortfolio,
  MetricsResponse,
  ModeResponse,
  OrderBookResponse,
  OrderBooksResponse,
  RiskSummary,
  StatusResponse,
  TwinSnapshot,
} from '../types'
import { useEffect, useState } from 'react'

// Polymarket Gamma API types
export interface PolymarketMeta {
  image: string
  question: string
  volume: string
  category: string
}

async function get<T>(path: string): Promise<T> {
  const res = await fetch(path, { signal: AbortSignal.timeout(10_000) })
  if (!res.ok) throw new Error(`HTTP ${res.status} ${path}`)
  return res.json() as Promise<T>
}

async function post<T>(path: string, body: unknown): Promise<T> {
  const res = await fetch(path, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
    signal: AbortSignal.timeout(15_000),
  })
  if (!res.ok) throw new Error(`HTTP ${res.status} ${path}`)
  return res.json() as Promise<T>
}

export function getPolymarketUrl(tokenId: string): string {
  return `https://polymarket.com/trade/${tokenId}`
}

export async function resolveMarketUrl(tokenId: string): Promise<string | null> {
  try {
    const data = await get<{ url: string | null }>(`/api/market-url/${tokenId}`)
    return data.url ?? null
  } catch {
    return null
  }
}

/**
 * Fetch Polymarket metadata from Gamma API (public, no auth required)
 * Returns market image, question, 24h volume, and category.
 */
export async function fetchPolymarketMeta(tokenId: string): Promise<PolymarketMeta | null> {
  try {
    const res = await fetch(
      `https://gamma-api.polymarket.com/markets?clob_token_ids=${tokenId}`,
      { signal: AbortSignal.timeout(8_000) }
    )
    if (!res.ok) return null
    const data = await res.json() as any
    if (!Array.isArray(data) || data.length === 0) return null
    const market = data[0]
    return {
      image: market.image ?? '',
      question: market.question ?? '',
      volume: market.volume_24h ?? '0',
      category: market.category ?? '',
    }
  } catch {
    return null
  }
}

/**
 * Hook to fetch Polymarket metadata for multiple token IDs
 * Dedupes requests and caches results per session
 */
const metaCache = new Map<string, Promise<PolymarketMeta | null>>()

export function useMarketMeta(tokenIds: string[]) {
  const [metadata, setMetadata] = useState<Map<string, PolymarketMeta>>(new Map())
  const [loading, setLoading] = useState(true)

  useEffect(() => {
    if (tokenIds.length === 0) {
      setMetadata(new Map())
      setLoading(false)
      return
    }

    let cancelled = false
    setLoading(true)

    Promise.all(
      tokenIds.map((id) => {
        if (!metaCache.has(id)) {
          metaCache.set(id, fetchPolymarketMeta(id))
        }
        return metaCache.get(id)!.then((m) => ({ id, meta: m }))
      })
    ).then((results) => {
      if (cancelled) return
      const map = new Map<string, PolymarketMeta>()
      results.forEach(({ id, meta }) => {
        if (meta) map.set(id, meta)
      })
      setMetadata(map)
      setLoading(false)
    }).catch(() => {
      if (!cancelled) setLoading(false)
    })

    return () => { cancelled = true }
  }, [tokenIds])

  return { metadata, loading }
}

export const api = {
  mode: () => get<ModeResponse>('/api/mode'),
  status: () => get<StatusResponse>('/api/status'),
  portfolio: () => get<FullPortfolio>('/api/portfolio'),
  history: (page = 1, perPage = 50) =>
    get<HistoryResponse>(`/api/history?page=${page}&per_page=${perPage}`),
  historyAll: () =>
    get<HistoryResponse>('/api/history?page=1&per_page=10000').then(r => r.trades),
  risk: () => get<RiskSummary>('/api/risk'),
  latency: () => get<LatencyResponse>('/api/latency'),
  failsafe: () => get<FailsafeSnapshot>('/api/failsafe'),
  fillWindow: () => get<FillWindowResponse>('/api/fill-window'),
  metrics: () => get<MetricsResponse>('/api/metrics'),
  livePortfolio: () => get<LivePortfolio>('/api/live/portfolio'),
  pause: (paused: boolean) => post<{ trading_paused: boolean }>('/api/pause', { paused }),
  resetCircuitBreaker: () => post<{ ok: boolean }>('/api/risk/reset_circuit_breaker', {}),
  sellPosition: (id: number, fraction = 1.0) =>
    post<{ ok: boolean; realized_pnl: number }>(`/api/positions/${id}/sell`, { fraction }),
  activity: () => get<{ entries: import('../types').ActivityEntry[] }>('/api/activity'),
  bullpenHealth: () => get<BullpenHealthResponse>('/api/bullpen/health'),
  bullpenDiscovery: () => get<BullpenDiscoveryResponse>('/api/bullpen/discovery'),
  bullpenConvergence: () => get<BullpenConvergenceResponse>('/api/bullpen/convergence'),
  orderbook: (tokenId: string) => get<OrderBookResponse>(`/api/orderbook/${tokenId}`),
  orderbooks: () => get<OrderBooksResponse>('/api/orderbooks'),
  twin: () => get<TwinSnapshot>('/api/twin'),
  updateConfig: (config: Record<string, number | boolean>) => post<{ ok: boolean; updated: string[] }>('/api/config', config),
  alpha: () => get<AlphaStatus>('/api/alpha'),
}

export type AlphaCycleMarket = {
  question: string
  yes_price: number
  llm_probability: number | null
  confidence: number | null
  edge_bps: number | null
  action: string
  reasoning: string | null
  spread_pct: number | null
  bid_depth_usdc: number | null
  ask_depth_usdc: number | null
  price_change_1h: number | null
  side: string | null
  token_id: string | null
  recommended_size_usdc: number | null
  reasoning_chain?: {
    call1_probability: number | null
    call2_probability: number | null
    final_probability: number | null
    combination_method: string | null
    category: string | null
    call1_reasoning: string | null
    call2_critique: string | null
    base_rate: string | null
    evidence_for: string[]
    evidence_against: string[]
    cognitive_biases: string[]
  }
}

export type AlphaSignalRecord = {
  timestamp: string
  analysis_id: string
  token_id: string
  market_question: string
  side: string
  confidence: number
  reasoning: string
  recommended_price: number
  recommended_size_usdc: number
  status: string
  position_id: number | null
  realized_pnl: number | null
  unrealized_pnl: number | null
  entry_price: number | null
  current_price: number | null
}

export type AlphaCycleSnapshot = {
  timestamp: string
  markets_scanned: number
  markets_analyzed: number
  signals_submitted: number
  signals_accepted: number
  cycle_duration_secs: number
}

export type AlphaPosition = {
  id: number
  token_id: string
  market_title: string | null
  side: string
  entry_price: number
  current_price: number
  shares: number
  usdc_spent: number
  unrealized_pnl: number
  unrealized_pnl_pct: number
  analysis_id: string | null
  duration_secs: number
  opened_at: string
}

export type AlphaClosedTrade = {
  token_id: string
  market_title: string | null
  side: string
  entry_price: number
  exit_price: number
  realized_pnl: number
  fees_paid_usdc: number
  reason: string
  duration_secs: number
  analysis_id: string | null
  closed_at: string
}

export type AlphaPerformance = {
  win_count: number
  loss_count: number
  win_rate_pct: number
  avg_pnl_per_trade: number
  best_trade_pnl: number
  worst_trade_pnl: number
  total_fees_paid: number
}

export type AlphaStatus = {
  enabled: boolean
  signals_received: number
  signals_accepted: number
  signals_rejected: number
  accept_rate_pct: number
  reject_reasons: Record<string, number>
  realized_pnl_usdc: number
  unrealized_pnl_usdc: number
  positions_opened: number
  positions_closed: number
  reason?: string
  // Cycle reporting
  cycles_completed: number
  last_cycle_at: string | null
  last_cycle_markets_scanned: number
  last_cycle_markets_analyzed: number
  last_cycle_signals_generated: number
  last_cycle_signals_submitted: number
  last_cycle_duration_secs: number
  last_cycle_top_markets: AlphaCycleMarket[]
  // New rich data
  signal_history: AlphaSignalRecord[]
  cycle_history: AlphaCycleSnapshot[]
  ai_positions: AlphaPosition[]
  ai_closed_trades: AlphaClosedTrade[]
  performance: AlphaPerformance
}
