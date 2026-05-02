import { useState, useEffect, useRef, useCallback } from 'react';

const BASE = '';

type JsonRecord = Record<string, unknown>;

const STRATEGY_MODES = ['mirror', 'conservative', 'aggressive'] as const;

export type StrategyMode = typeof STRATEGY_MODES[number];

export type StrategyStatus = {
  mode: StrategyMode;
  switch_seq?: number;
  last_switched_at_ms?: number;
  cooldown_secs?: number;
  runtime_switch_enabled: boolean;
  live_switch_allowed?: boolean;
  switch_allowed: boolean;
  cooldown_remaining_secs?: number;
  lockout_reason?: string;
  lockout_until_ts?: number;
  updated_at_ms?: number;
};

export type StrategyHistoryEntry = {
  seq?: number;
  switched_at_ms: number;
  to_mode: StrategyMode;
  from_mode?: StrategyMode;
  reason?: string;
  source?: string;
};

export type StrategyHistory = {
  entries: StrategyHistoryEntry[];
};

export type WebSocketSnapshot = {
  type?: string;
  timestamp_ms?: number;
  snapshot_seq?: number;
  engine_uptime_secs?: number;
  portfolio_age_ms?: number;
  ws_connected?: boolean;
  trading_paused?: boolean;
  messages_total?: number;
  portfolio?: JsonRecord;
  recent_activity?: ReadonlyArray<JsonRecord>;
  strategy?: StrategyStatus;
  strategy_history?: StrategyHistoryEntry[];
  [key: string]: unknown;
};

const isRecord = (value: unknown): value is JsonRecord =>
  typeof value === 'object' && value !== null;

const asNumber = (value: unknown): number | undefined =>
  typeof value === 'number' ? value : undefined;

const asString = (value: unknown): string | undefined =>
  typeof value === 'string' ? value : undefined;

const asBoolean = (value: unknown): boolean | undefined =>
  typeof value === 'boolean' ? value : undefined;

const asStrategyMode = (value: unknown): StrategyMode | undefined =>
  typeof value === 'string' && STRATEGY_MODES.includes(value as StrategyMode)
    ? (value as StrategyMode)
    : undefined;

const parseStrategyStatus = (value: unknown): StrategyStatus | null => {
  if (!isRecord(value)) return null;

  const mode = asStrategyMode(value.mode ?? value.strategy_mode ?? value.current_mode);
  if (!mode) return null;

  const switchSeq = asNumber(value.switch_seq);
  const lastSwitchedAtMs = asNumber(value.last_switched_at_ms ?? value.switched_at_ms);
  const cooldownSecs = asNumber(value.cooldown_secs ?? value.switch_cooldown_secs);
  const runtimeSwitchEnabled = asBoolean(
    value.runtime_switch_enabled ?? value.runtime_switching_enabled ?? value.runtime_switch_enabled_by_config,
  );
  const liveSwitchAllowed = asBoolean(value.live_switch_allowed);
  const switchAllowed = asBoolean(value.switch_allowed ?? value.can_switch);
  const cooldownRemaining = asNumber(value.cooldown_remaining_secs ?? value.cooldown_remaining_ms)
    ?? (asNumber(value.cooldown_remaining_ms) !== undefined
      ? ((asNumber(value.cooldown_remaining_ms) ?? 0) / 1000)
      : undefined);
  const lockoutUntil = asNumber(value.lockout_until_ts ?? value.lockout_until_ms);
  const updatedAt = asNumber(value.updated_at_ms ?? value.updated_at_ts);
  const lockoutReason = asString(value.lockout_reason ?? value.reason);

  return {
    mode,
    ...(switchSeq !== undefined ? { switch_seq: switchSeq } : {}),
    ...(lastSwitchedAtMs !== undefined ? { last_switched_at_ms: lastSwitchedAtMs } : {}),
    ...(cooldownSecs !== undefined ? { cooldown_secs: cooldownSecs } : {}),
    runtime_switch_enabled: runtimeSwitchEnabled ?? false,
    ...(liveSwitchAllowed !== undefined ? { live_switch_allowed: liveSwitchAllowed } : {}),
    switch_allowed: switchAllowed ?? ((runtimeSwitchEnabled ?? false) && (liveSwitchAllowed ?? true)),
    ...(cooldownRemaining !== undefined ? { cooldown_remaining_secs: cooldownRemaining } : {}),
    ...(lockoutReason ? { lockout_reason: lockoutReason } : {}),
    ...(lockoutUntil !== undefined ? { lockout_until_ts: lockoutUntil } : {}),
    ...(updatedAt !== undefined ? { updated_at_ms: updatedAt } : {}),
  };
};

const parseStrategyHistory = (value: unknown): StrategyHistoryEntry[] | undefined => {
  if (!Array.isArray(value)) return undefined;

  const entries = value.flatMap((entry): StrategyHistoryEntry[] => {
    if (!isRecord(entry)) return [];
    const toMode = asStrategyMode(entry.to_mode ?? entry.to ?? entry.mode);
    if (!toMode) return [];

    const switchedAtMs = asNumber(entry.switched_at_ms ?? entry.timestamp_ms ?? entry.timestamp);
    if (switchedAtMs === undefined) return [];

    const fromMode = asStrategyMode(entry.from_mode ?? entry.from);
    const seq = asNumber(entry.seq ?? entry.switch_seq);
    const reason = asString(entry.reason);
    const source = asString(entry.source ?? entry.actor);

    return [{
      ...(seq !== undefined ? { seq } : {}),
      switched_at_ms: switchedAtMs,
      to_mode: toMode,
      ...(fromMode ? { from_mode: fromMode } : {}),
      ...(reason ? { reason } : {}),
      ...(source ? { source } : {}),
    }];
  });

  return entries.length > 0 ? entries : undefined;
};

const parseWebSocketSnapshot = (value: unknown): WebSocketSnapshot | null => {
  if (!isRecord(value)) return null;

  const snapshot: WebSocketSnapshot = { ...value };
  if (isRecord(value.portfolio)) snapshot.portfolio = value.portfolio;

  if (Array.isArray(value.recent_activity)) {
    snapshot.recent_activity = value.recent_activity.filter(isRecord);
  }

  const strategyFromNested = parseStrategyStatus(value.strategy);
  const strategyFromTopLevel = parseStrategyStatus(value);
  if (strategyFromNested ?? strategyFromTopLevel) {
    snapshot.strategy = strategyFromNested ?? strategyFromTopLevel ?? undefined;
  }

  const historyFromNested = isRecord(value.strategy) ? parseStrategyHistory(value.strategy.history) : undefined;
  const history = parseStrategyHistory(value.strategy_history) ?? historyFromNested;
  if (history) snapshot.strategy_history = history;

  return snapshot;
};

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

export type HistoryPage = {
  trades: unknown[];
  total: number;
  page: number;
  per_page: number;
  total_pages: number;
};

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
  const [snapshot, setSnapshot] = useState<WebSocketSnapshot | null>(null);
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
        try {
          const parsed = parseWebSocketSnapshot(JSON.parse(e.data as string));
          if (parsed) setSnapshot(parsed);
        } catch {
          // ignore malformed
        }
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

export type SetStrategyModeResponse = {
  ok: boolean;
  strategy?: StrategyStatus;
  strategy_history?: StrategyHistoryEntry[];
  error?: string;
};

export async function postStrategy(mode: StrategyMode, reason = 'web-dashboard-switch'): Promise<SetStrategyModeResponse> {
  try {
    const res = await fetch(`${BASE}/api/strategy`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ mode, reason }),
    });

    const text = await res.text();
    if (!text) {
      return { ok: false, error: `HTTP ${res.status}: Empty response` };
    }

    let json: unknown;
    try {
      json = JSON.parse(text);
    } catch {
      return { ok: false, error: `HTTP ${res.status}: Bad response` };
    }

    if (!isRecord(json)) {
      return { ok: false, error: `HTTP ${res.status}: Unexpected payload` };
    }

    const strategy = parseStrategyStatus(json.strategy ?? json) ?? undefined;
    const nestedHistory = isRecord(json.strategy) ? parseStrategyHistory(json.strategy.history) : undefined;
    const strategyHistory = parseStrategyHistory(json.strategy_history) ?? nestedHistory;
    const apiError = asString(json.error ?? json.message);
    const apiOk = asBoolean(json.ok);
    const ok = (apiOk ?? res.ok) && !apiError;

    return {
      ok,
      ...(strategy ? { strategy } : {}),
      ...(strategyHistory ? { strategy_history: strategyHistory } : {}),
      ...(apiError ? { error: apiError } : {}),
    };
  } catch (e: unknown) {
    return { ok: false, error: (e as Error).message };
  }
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
