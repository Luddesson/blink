import { useState, useEffect, useRef, useCallback } from 'react';

const BASE = '';

// paused=true stops the interval (used when a WS feed already covers the data).
export function useFetch<T>(path: string, intervalMs = 2000, paused = false) {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (paused) return;
    let active = true;
    const fetchData = async () => {
      try {
        const res = await fetch(`${BASE}${path}`, {
          signal: AbortSignal.timeout(Math.max(intervalMs - 500, 1000)),
        });
        if (!res.ok) throw new Error(`HTTP ${res.status}`);
        const json = await res.json();
        if (active) { setData(json); setError(null); }
      } catch (e: unknown) {
        if (active && (e as Error).name !== 'AbortError') setError((e as Error).message);
      }
    };
    fetchData();
    const id = setInterval(fetchData, intervalMs);
    return () => { active = false; clearInterval(id); };
  }, [path, intervalMs, paused]);

  return { data, error };
}

// ─── Paginaged history fetch ──────────────────────────────────────────────────

export interface HistoryPage {
  trades: unknown[];
  total: number;
  page: number;
  per_page: number;
  total_pages: number;
}

export function useHistory(page: number, perPage = 50) {
  const [data, setData] = useState<HistoryPage | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const res = await fetch(`${BASE}/api/history?page=${page}&per_page=${perPage}`);
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const json = await res.json();
      setData(json);
      setError(null);
    } catch (e: unknown) {
      setError((e as Error).message);
    } finally {
      setLoading(false);
    }
  }, [page, perPage]);

  useEffect(() => { load(); }, [load]);

  return { data, loading, error, reload: load };
}

export function useWebSocket() {
  const [snapshot, setSnapshot] = useState<unknown>(null);
  const [connected, setConnected] = useState(false);
  const wsRef = useRef<WebSocket | null>(null);

  useEffect(() => {
    let active = true;
    const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    const wsHost = window.location.port === '5173'
      ? `${window.location.hostname}:3030`
      : window.location.host;
    const url = `${protocol}//${wsHost}/ws`;

    function connect() {
      if (!active) return;
      const ws = new WebSocket(url);
      wsRef.current = ws;
      ws.onopen = () => { if (active) setConnected(true); };
      ws.onclose = () => {
        if (active) {
          setConnected(false);
          setTimeout(connect, 3000);
        }
      };
      ws.onmessage = (e) => {
        if (!active) return;
        try { setSnapshot(JSON.parse(e.data as string)); } catch { /* ignore malformed */ }
      };
    }
    connect();

    return () => {
      active = false;
      wsRef.current?.close();
    };
  }, []);

  return { snapshot, connected };
}

export async function postPause(paused: boolean) {
  await fetch(`${BASE}/api/pause`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ paused }),
  });
}

export async function postSellPosition(positionId: number, fraction: number): Promise<{ ok: boolean; realized_pnl: number; error?: string }> {
  const res = await fetch(`${BASE}/api/positions/${positionId}/sell`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ fraction }),
  });
  const text = await res.text();
  if (!text) return { ok: false, realized_pnl: 0, error: 'Empty response — is the engine running?' };
  try {
    return JSON.parse(text);
  } catch {
    return { ok: false, realized_pnl: 0, error: `Bad response: ${text.slice(0, 80)}` };
  }
}

export async function prepareSettlement(amount_usdc: number, recipient?: string, token = 'USDC', positionId?: number) {
  const res = await fetch(`${BASE}/api/wallet/prepare_settlement`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ amount_usdc, recipient: recipient || '', token, position_id: positionId }),
  });
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  return res.json();
}

export async function submitSignedTx(payload: unknown) {
  const res = await fetch(`${BASE}/api/wallet/submit_signed_tx`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(payload),
  });
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  return res.json();
}

// ─── Alpha sidecar status ─────────────────────────────────────────────────────

export type AlphaSignalRecord = {
  timestamp: string;
  analysis_id: string;
  token_id: string;
  market_question: string;
  side: string;
  confidence: number;
  reasoning: string;
  recommended_price: number;
  recommended_size_usdc: number;
  status: string;
  position_id?: number;
  entry_price?: number;
  current_price?: number;
  realized_pnl?: number;
  unrealized_pnl?: number;
};

export type AlphaCycleMarket = {
  question: string;
  yes_price: number;
  llm_probability?: number;
  confidence?: number;
  edge_bps?: number;
  action: string;
  reasoning?: string;
  side?: string;
  token_id: string;
  reasoning_chain?: {
    call1_probability?: number;
    call2_probability?: number;
    final_probability?: number;
    combination_method?: string;
    category?: string;
    call1_reasoning?: string;
    call2_critique?: string;
    base_rate?: string;
    evidence_for?: string[];
    evidence_against?: string[];
    cognitive_biases?: string[];
  };
};

export type AlphaPosition = {
  id: number;
  token_id: string;
  market_title: string;
  side: string;
  entry_price: number;
  current_price: number;
  shares: number;
  usdc_spent: number;
  unrealized_pnl: number;
  unrealized_pnl_pct: number;
  analysis_id: string;
  duration_secs: number;
};

export type AlphaStatus = {
  enabled: boolean;
  signals_received: number;
  signals_accepted: number;
  signals_rejected: number;
  accept_rate_pct: number;
  reject_reasons: Record<string, number>;
  realized_pnl_usdc: number;
  unrealized_pnl_usdc: number;
  positions_opened: number;
  positions_closed: number;
  reason?: string;
  // Cycle info
  cycles_completed?: number;
  last_cycle_at?: string;
  last_cycle_markets_scanned?: number;
  last_cycle_markets_analyzed?: number;
  last_cycle_signals_submitted?: number;
  last_cycle_duration_secs?: number;
  last_cycle_top_markets?: AlphaCycleMarket[];
  // History
  signal_history?: AlphaSignalRecord[];
  ai_positions?: AlphaPosition[];
  ai_closed_trades?: Array<{
    token_id: string;
    market_title: string;
    side: string;
    entry_price: number;
    exit_price: number;
    realized_pnl: number;
    reason: string;
    duration_secs: number;
    closed_at: string;
  }>;
  // Performance
  performance?: {
    win_count: number;
    loss_count: number;
    win_rate_pct: number;
    avg_pnl_per_trade: number;
    best_trade_pnl: number;
    worst_trade_pnl: number;
    total_fees_paid: number;
  };
  // Calibration
  calibration?: unknown;
};

export function useAlpha(intervalMs = 5000) {
  return useFetch<AlphaStatus>('/api/alpha', intervalMs);
}
