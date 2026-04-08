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

async function get<T>(path: string): Promise<T> {
  const res = await fetch(path)
  if (!res.ok) throw new Error(`HTTP ${res.status} ${path}`)
  return res.json() as Promise<T>
}

async function post<T>(path: string, body: unknown): Promise<T> {
  const res = await fetch(path, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  })
  if (!res.ok) throw new Error(`HTTP ${res.status} ${path}`)
  return res.json() as Promise<T>
}

export const api = {
  mode: () => get<ModeResponse>('/api/mode'),
  status: () => get<StatusResponse>('/api/status'),
  portfolio: () => get<FullPortfolio>('/api/portfolio'),
  history: (page = 1, perPage = 50) =>
    get<HistoryResponse>(`/api/history?page=${page}&per_page=${perPage}`),
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
  updateConfig: (config: Record<string, number>) => post<{ ok: boolean; updated: string[] }>('/api/config', config),
}
