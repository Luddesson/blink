import { useState, useRef, useEffect, useMemo } from 'react';
import {
  useFetch,
  useWebSocket,
  postPause,
  postSellPosition,
  prepareSettlement,
  submitSignedTx,
  postStrategy,
  type StrategyMode,
} from '../hooks/useApi';
import { Card, Stat, Badge } from '../components/Card';
import { EquityChart } from '../components/EquityChart';

// ─── Uptime helper ────────────────────────────────────────────────────────────

function formatUptime(secs: number): string {
  if (secs < 60) return `${secs}s`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m`;
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  return m > 0 ? `${h}h ${m}m` : `${h}h`;
}

function formatAge(secs: number): string {
  if (secs < 60) return `${secs}s`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m ${secs % 60}s`;
  return `${Math.floor(secs / 3600)}h ${Math.floor((secs % 3600) / 60)}m`;
}

const STRATEGY_MODES: readonly StrategyMode[] = ['mirror', 'conservative', 'aggressive'];

const STRATEGY_BADGE_VARIANTS: Record<StrategyMode, 'green' | 'yellow' | 'red'> = {
  mirror: 'green',
  conservative: 'yellow',
  aggressive: 'red',
};

// ─── Sell Modal ───────────────────────────────────────────────────────────────

type SellModalProps = {
  pos: any;
  onClose: () => void;
  onSold: () => void;
};

function SellModal({ pos, onClose, onSold }: SellModalProps) {
  const [loading, setLoading] = useState(false);
  const [result, setResult] = useState<string | null>(null);
  const [settlementPayload, setSettlementPayload] = useState<any | null>(null);
  const [signedTxInput, setSignedTxInput] = useState<string>('');
  const [submitResult, setSubmitResult] = useState<string | null>(null);

  const sell = async (fraction: number) => {
    setLoading(true);
    try {
      const res = await postSellPosition(pos.id, fraction);
      if (res.ok) {
        const pnl = res.realized_pnl ?? 0;
        setResult(`✅ Sold ${fraction * 100}% — P&L: ${pnl >= 0 ? '+' : ''}$${pnl.toFixed(2)}`);
        // Prepare unsigned settlement payload for client-side signing with Phantom
        try {
          const payload = await prepareSettlement(pnl, undefined, 'USDC', pos.id);
          setSettlementPayload(payload.unsigned_tx ?? payload);
        } catch (e: any) {
          setSettlementPayload({ error: `prepare failed: ${e.message}` });
        }
        // Do not auto-close; let user sign/submit
      } else {
        setResult(`❌ Error: ${res.error ?? 'unknown'}`);
        setLoading(false);
      }
    } catch (e: any) {
      setResult(`❌ Network error: ${e.message}`);
      setLoading(false);
    }
  };

  const submitSigned = async () => {
    setSubmitResult(null);
    try {
      const signedTxRaw = signedTxInput.trim();
      if (!signedTxRaw) {
        setSubmitResult('ERROR: Paste a signed tx before submitting.');
        return;
      }

      let signedTx: unknown = signedTxRaw;
      try {
        signedTx = JSON.parse(signedTxRaw);
      } catch {
        // Keep the raw text so non-JSON payloads can still be submitted.
      }

      const resp = await submitSignedTx({ chain: settlementPayload?.chain ?? 'solana', signed_tx: signedTx, position_id: pos.id });
      setSubmitResult(`OK tx_id=${resp.tx_id}`);
      // After successful submission, mark sold + close modal
      setTimeout(() => { onSold(); onClose(); }, 900);
    } catch (e: any) {
      setSubmitResult(`ERROR: ${e.message}`);
    }
  };

  const title = pos.market_title || pos.token_id.slice(0, 30) + '…';
  const outcome = pos.market_outcome ? ` — ${pos.market_outcome}` : '';
  const pnlColor = pos.unrealized_pnl >= 0 ? 'text-emerald-400' : 'text-red-400';

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/70" onClick={onClose}>
      <div
        className="bg-gray-900 border border-gray-700 rounded-xl p-6 w-full max-w-sm shadow-2xl"
        onClick={e => e.stopPropagation()}
      >
        <h2 className="text-white font-bold text-base mb-1">Sell position #{pos.id}</h2>
        <p className="text-gray-400 text-xs mb-1 truncate">{title}{outcome}</p>
        <div className="flex gap-3 text-xs mb-4">
          <span className="text-gray-500">Entry <span className="text-white">{pos.entry_price.toFixed(3)}</span></span>
          <span className="text-gray-500">Now <span className="text-white">{pos.current_price.toFixed(3)}</span></span>
          <span className={`font-semibold ${pnlColor}`}>
            {pos.unrealized_pnl >= 0 ? '+' : ''}${pos.unrealized_pnl.toFixed(2)}
          </span>
        </div>

        {result ? (
          <div>
            <p className="text-center text-sm py-2 font-semibold text-white">{result}</p>
            {settlementPayload && (
              <div className="bg-gray-800 p-2 rounded mt-2 text-xs text-gray-300">
                <div className="mb-1 font-semibold text-white">Unsigned settlement payload (sign in Phantom):</div>
                <pre className="text-xs max-h-40 overflow-auto">{JSON.stringify(settlementPayload, null, 2)}</pre>
                <div className="mt-2">
                  <label className="text-gray-400 text-xs">Paste signed tx JSON or raw text to submit:</label>
                  <textarea
                    value={signedTxInput}
                    onChange={e => setSignedTxInput(e.target.value)}
                    className="w-full bg-gray-900 border border-gray-700 rounded p-2 text-xs mt-1"
                    rows={4}
                    placeholder='{"signature":"..."}'
                  />
                  <div className="flex gap-2 mt-2">
                    <button onClick={submitSigned} className="px-3 py-1 rounded bg-emerald-700 text-white text-xs">Submit signed tx</button>
                    <button onClick={() => { setSettlementPayload(null); setResult(null); setLoading(false); }} className="px-3 py-1 rounded border border-gray-700 text-xs">Dismiss</button>
                  </div>
                  {submitResult && <div className="mt-2 text-sm text-white">{submitResult}</div>}
                </div>
              </div>
            )}
          </div>
        ) : (
          <>
            <p className="text-gray-500 text-xs mb-3">Choose how much to sell:</p>
            <div className="grid grid-cols-4 gap-2">
              {[0.25, 0.50, 0.75, 1.0].map(f => (
                <button
                  key={f}
                  disabled={loading}
                  onClick={() => sell(f)}
                  className={`py-2 rounded-lg text-sm font-bold border transition-colors
                    ${f === 1.0
                      ? 'border-red-600 text-red-400 hover:bg-red-900/40'
                      : 'border-gray-600 text-gray-200 hover:bg-gray-700/60'}
                    disabled:opacity-40 disabled:cursor-not-allowed`}
                >
                  {f * 100}%
                </button>
              ))}
            </div>
          </>
        )}

        <button
          onClick={onClose}
          className="mt-4 w-full py-1.5 rounded text-xs text-gray-500 hover:text-gray-300 hover:bg-gray-800 transition-colors"
        >
          Cancel
        </button>
      </div>
    </div>
  );
}

export default function Dashboard() {
  const { snapshot, connected } = useWebSocket();
  const wsPortfolio = snapshot?.portfolio ?? null;
  const wsConnected = connected && wsPortfolio != null;

  // Fallback REST polling — only active before WS delivers first portfolio data.
  // Fully paused once WS is live; WS snapshot already contains all portfolio fields.
  const { data: portfolio } = useFetch<Record<string, unknown>>(
    '/api/portfolio',
    3_000,
    wsConnected   // pause completely when WS covers it
  );

  // Activity log: read from WS recent_activity when possible, otherwise REST-poll slowly.
  const wsActivity = snapshot?.recent_activity;
  const { data: activityData } = useFetch<{ entries: unknown[] }>(
    '/api/activity',
    wsConnected ? 30_000 : 3_000,
    wsConnected   // fully pause REST once WS is live
  );

  const [sellPos, setSellPos] = useState<Record<string, unknown> | null>(null);
  const [interval, setInterval] = useState<string>('1h');
  const [sortKey, setSortKey] = useState<string>('unrealized_pnl');
  const [sortDir, setSortDir] = useState<'asc' | 'desc'>('desc');
  const [search, setSearch] = useState<string>('');
  const [strategyMode, setStrategyMode] = useState<StrategyMode>('mirror');
  const [strategySwitching, setStrategySwitching] = useState(false);
  const [strategyError, setStrategyError] = useState<string | null>(null);
  const activityRef = useRef<HTMLDivElement>(null);

  // Merge WS and REST data — WS wins when available.
  // Note: wsPortfolio may exist but be empty on first connect; fall back to REST.
  const liveStats = (wsPortfolio && Object.keys(wsPortfolio).length > 0) ? wsPortfolio : portfolio;
  const p = liveStats;

  // Prefer WS recent_activity when it has entries; otherwise fall back to REST.
  // Using `&&` length check because [] is truthy so ?? wouldn't fall back correctly.
  const activityEntries: unknown[] =
    (Array.isArray(wsActivity) && wsActivity.length > 0)
      ? wsActivity
      : (Array.isArray(activityData?.entries) ? activityData.entries : []);

  const strategyStatus = snapshot?.strategy;
  const activeStrategyMode = strategyStatus?.mode ?? strategyMode;
  const runtimeSwitchEnabled = strategyStatus?.runtime_switch_enabled ?? false;
  const strategySwitchAllowed = strategyStatus?.switch_allowed ?? false;
  const cooldownSecs = strategyStatus?.cooldown_remaining_secs;
  const lockoutReason = strategyStatus?.lockout_reason;
  const lockoutUntil = strategyStatus?.lockout_until_ts;
  const canSwitchStrategy = connected && runtimeSwitchEnabled && strategySwitchAllowed;
  const strategySelectDisabled = !canSwitchStrategy || strategySwitching;

  useEffect(() => {
    if (strategyStatus?.mode) {
      setStrategyMode(strategyStatus.mode);
      setStrategyError(null);
    }
  }, [strategyStatus?.mode]);

  // Auto-scroll activity log to bottom when new entries arrive
  useEffect(() => {
    if (activityRef.current) {
      activityRef.current.scrollTop = activityRef.current.scrollHeight;
    }
  }, [activityEntries.length]);

  // Build equity curve filtered by selected time interval
  const INTERVALS: Record<string, { label: string; ms: number | null }> = {
    '1m':  { label: '1m',  ms: 60_000 },
    '5m':  { label: '5m',  ms: 5 * 60_000 },
    '15m': { label: '15m', ms: 15 * 60_000 },
    '30m': { label: '30m', ms: 30 * 60_000 },
    '1h':  { label: '1h',  ms: 60 * 60_000 },
    '4h':  { label: '4h',  ms: 4 * 60 * 60_000 },
    '1d':  { label: '1d',  ms: 24 * 60 * 60_000 },
    '1w':  { label: '1w',  ms: null },
  };

  // Memoize so canvas only redraws when interval or data actually changes
  const equityCurve = useMemo(() => {
    const rawCandidate = (wsPortfolio?.equity_curve ?? portfolio?.equity_curve ?? []) as unknown;
    const tsCandidate = (wsPortfolio?.equity_timestamps ?? portfolio?.equity_timestamps ?? []) as unknown;
    const raw: number[] = Array.isArray(rawCandidate) ? rawCandidate.filter((v): v is number => typeof v === 'number') : [];
    const ts: number[] = Array.isArray(tsCandidate) ? tsCandidate.filter((v): v is number => typeof v === 'number') : [];
    const windowMs = INTERVALS[interval]?.ms;
    const now = Date.now();
    const hasTimestamps = ts.some(t => t != null && t > 0);
    let peak = 0;
    return raw
      .map((nav: number, i: number) => ({ nav, ts: ts[i] ?? null }))
      .filter(({ ts: t }) => !windowMs || (!hasTimestamps) || (t !== null && (now - t) <= windowMs))
      .map(({ nav, ts: t }) => {
        peak = Math.max(peak, nav);
        const drawdown = peak > 0 ? -((peak - nav) / peak) * 100 : 0;
        return {
          nav,
          drawdown,
          time: t ? new Date(t).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }) : '',
        };
      });
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [wsPortfolio?.equity_curve, portfolio?.equity_curve, interval]);

  const toggleSort = (key: string) => {
    if (sortKey === key) setSortDir(d => d === 'desc' ? 'asc' : 'desc');
    else { setSortKey(key); setSortDir('desc'); }
  };
  const sortIcon = (key: string) => sortKey === key ? (sortDir === 'desc' ? ' ↓' : ' ↑') : '';

  type PositionRow = Record<string, unknown>;
  // Use WS positions if available (live), else fall back to REST portfolio
  const openPositions = wsPortfolio?.open_positions ?? p?.open_positions;
  const allPositions: PositionRow[] = (
    Array.isArray(openPositions) ? (openPositions as PositionRow[]) : []
  ).filter((pos) => typeof pos.shares === 'number' && (pos.shares as number) >= 0.01);
  const filteredPositions = search
    ? allPositions.filter((pos) =>
        `${(pos.market_title as string | undefined) ?? (pos.token_id as string)} ${(pos.market_outcome as string | undefined) ?? ''}`
          .toLowerCase()
          .includes(search.toLowerCase()))
    : allPositions;
  const sortedPositions = [...filteredPositions].sort((a, b) => {
    const va = a[sortKey];
    const vb = b[sortKey];
    const dir = sortDir === 'desc' ? -1 : 1;
    if (typeof va === 'string') return dir * va.localeCompare((vb as string) ?? '');
    return dir * (((va as number) ?? 0) - ((vb as number) ?? 0));
  });

  const totalPnl = ((liveStats?.realized_pnl_usdc as number) ?? 0) + ((liveStats?.unrealized_pnl_usdc as number) ?? 0);

  const snap = snapshot;
  const snapPaused = snap?.trading_paused as boolean | undefined;
  const handlePause = () => {
    if (snap) postPause(!snapPaused);
  };

  const handleStrategyChange = async (nextModeRaw: string) => {
    if (!STRATEGY_MODES.includes(nextModeRaw as StrategyMode)) return;
    const nextMode = nextModeRaw as StrategyMode;
    setStrategyMode(nextMode);
    setStrategyError(null);
    setStrategySwitching(true);
    const result = await postStrategy(nextMode);
    setStrategySwitching(false);

    if (!result.ok) {
      setStrategyError(result.error ?? 'Failed to switch strategy mode');
      if (strategyStatus?.mode) setStrategyMode(strategyStatus.mode);
      return;
    }

    if (result.strategy?.mode) {
      setStrategyMode(result.strategy.mode);
    }
  };

  const strategyHints: string[] = [];
  if (!connected) {
    strategyHints.push('Disconnected — strategy switching unavailable.');
  }
  if (!runtimeSwitchEnabled) {
    strategyHints.push('Runtime switching is disabled by engine config.');
  }
  if (typeof cooldownSecs === 'number' && cooldownSecs > 0) {
    strategyHints.push(`Cooldown active: ${Math.ceil(cooldownSecs)}s remaining.`);
  }
  if (lockoutReason) {
    strategyHints.push(`Lockout: ${lockoutReason}`);
  }
  if (typeof lockoutUntil === 'number') {
    const lockoutDate = lockoutUntil > 1_000_000_000_000
      ? new Date(lockoutUntil)
      : new Date(lockoutUntil * 1000);
    strategyHints.push(`Lockout until ${lockoutDate.toLocaleTimeString()}.`);
  }
  if (runtimeSwitchEnabled && !strategySwitchAllowed && strategyHints.length === 0) {
    strategyHints.push('Strategy switching is currently not allowed.');
  }

  // Helper to safely read numbers from liveStats (which is Record<string, unknown>)
  const n = (key: string): number => (liveStats?.[key] as number) ?? 0;

  return (
    <div className="space-y-4">
      {/* Status bar */}
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-3">
          <h1 className="text-xl font-bold text-white">Blink Engine</h1>
          <Badge
            text={(snap?.ws_connected as boolean) ? 'WS LIVE' : 'WS DOWN'}
            variant={(snap?.ws_connected as boolean) ? 'green' : 'red'}
          />
          <Badge
            text={connected ? 'UI CONNECTED' : 'UI DISCONNECTED'}
            variant={connected ? 'green' : 'gray'}
          />
          {snapPaused && <Badge text="PAUSED" variant="yellow" />}
        </div>
        <div className="flex items-center gap-3">
          <span className="text-xs text-gray-500">
            msgs: {((snap?.messages_total as number) || 0).toLocaleString()}
          </span>
          <button
            onClick={handlePause}
            className={`px-3 py-1 rounded text-xs font-semibold border ${
              snapPaused
                ? 'border-emerald-600 text-emerald-400 hover:bg-emerald-900/30'
                : 'border-yellow-600 text-yellow-400 hover:bg-yellow-900/30'
            }`}
          >
            {snapPaused ? 'RESUME' : 'PAUSE'}
          </button>
        </div>
      </div>

      {/* Portfolio overview */}
      <Card title="Strategy Mode">
        <div className="flex flex-col gap-3 md:flex-row md:items-center md:justify-between">
          <div className="flex flex-wrap items-center gap-2">
            <Badge text={activeStrategyMode.toUpperCase()} variant={STRATEGY_BADGE_VARIANTS[activeStrategyMode]} />
            <Badge text={runtimeSwitchEnabled ? 'RUNTIME SWITCH ON' : 'RUNTIME SWITCH OFF'} variant={runtimeSwitchEnabled ? 'green' : 'gray'} />
          </div>
          <div className="flex items-center gap-2">
            <label htmlFor="strategy-mode" className="text-xs text-gray-400">Mode</label>
            <select
              id="strategy-mode"
              value={strategyMode}
              onChange={(e) => handleStrategyChange(e.target.value)}
              disabled={strategySelectDisabled}
              className="rounded border border-gray-700 bg-gray-800 px-2 py-1 text-xs text-white disabled:cursor-not-allowed disabled:opacity-50"
            >
              {STRATEGY_MODES.map((modeOption) => (
                <option key={modeOption} value={modeOption}>
                  {modeOption}
                </option>
              ))}
            </select>
          </div>
        </div>
        {strategyHints.length > 0 && (
          <div className="mt-2 space-y-1 text-xs text-yellow-400">
            {strategyHints.map((hint) => (
              <div key={hint}>{hint}</div>
            ))}
          </div>
        )}
        {strategyError && <div className="mt-2 text-xs text-red-400">{strategyError}</div>}
      </Card>

      {/* Portfolio overview */}
      <div className="grid grid-cols-3 md:grid-cols-5 lg:grid-cols-10 gap-3">
        <Card>
          <Stat label="NAV" value={`$${n('nav_usdc').toFixed(2)}`} color="text-white" />
        </Card>
        <Card>
          <Stat label="Cash" value={`$${n('cash_usdc').toFixed(2)}`} />
        </Card>
        <Card>
          <Stat label="Invested" value={`$${n('invested_usdc').toFixed(2)}`} />
        </Card>
        <Card>
          <Stat
            label="Total P&L"
            value={`${totalPnl >= 0 ? '+' : ''}$${totalPnl.toFixed(2)}`}
            color={totalPnl >= 0 ? 'text-emerald-400' : 'text-red-400'}
          />
        </Card>
        <Card>
          <Stat
            label="Unrealized"
            value={`${n('unrealized_pnl_usdc') >= 0 ? '+' : ''}$${n('unrealized_pnl_usdc').toFixed(2)}`}
            color={n('unrealized_pnl_usdc') >= 0 ? 'text-emerald-400' : 'text-red-400'}
          />
        </Card>
        <Card>
          <Stat
            label="Realized"
            value={`${n('realized_pnl_usdc') >= 0 ? '+' : ''}$${n('realized_pnl_usdc').toFixed(2)}`}
            color={n('realized_pnl_usdc') >= 0 ? 'text-emerald-400' : 'text-red-400'}
          />
        </Card>
        <Card>
          <Stat label="Fees" value={`$${n('fees_paid_usdc').toFixed(2)}`} color="text-yellow-400" />
        </Card>
        <Card>
          <Stat label="Win Rate" value={`${n('win_rate_pct').toFixed(1)}%`} color="text-emerald-400" />
        </Card>
        <Card>
          <Stat label="Fill Rate" value={`${n('fill_rate_pct').toFixed(1)}%`} color="text-cyan-400" />
        </Card>
        <Card>
          <Stat label="Uptime" value={formatUptime(n('uptime_secs'))} color="text-gray-300" />
        </Card>
      </div>

      {/* Equity curve + Activity log */}
      <div className="grid grid-cols-1 lg:grid-cols-3 gap-4">
        <Card title="Equity Curve" className="lg:col-span-2">
          {/* Interval selector */}
          <div className="flex gap-1 mb-2">
            {Object.keys(INTERVALS).map(key => (
              <button
                key={key}
                onClick={() => setInterval(key)}
                className={`px-2 py-0.5 rounded text-xs font-semibold transition-colors ${
                  interval === key
                    ? 'bg-emerald-700 text-white'
                    : 'text-gray-500 hover:text-gray-300 hover:bg-gray-800'
                }`}
              >
                {key}
              </button>
            ))}
          </div>
          {equityCurve.length > 1 ? (
            <EquityChart data={equityCurve} height={200} />
          ) : (
            <div className="h-[200px] flex items-center justify-center text-gray-600">No data yet</div>
          )}
        </Card>

        <Card title="Activity Log">
          <div ref={activityRef} className="h-[220px] overflow-y-auto space-y-1 text-xs">
            {(activityEntries as Array<{timestamp?: string; kind?: string; message?: string}>).map((e, i) => (
              <div key={i} className="flex gap-2">
                <span className="text-gray-600 shrink-0">{e.timestamp ?? '-'}</span>
                <span className={
                  e.kind === 'Fill' ? 'text-emerald-400' :
                  e.kind === 'Signal' ? 'text-cyan-400' :
                  e.kind === 'Abort' ? 'text-red-400' :
                  e.kind === 'Skip' || e.kind === 'Warn' ? 'text-yellow-400' :
                  'text-gray-400'
                }>{e.message ?? ''}</span>
              </div>
            ))}
            {activityEntries.length === 0 && (
              <div className="text-gray-600">No activity yet</div>
            )}
          </div>
        </Card>
      </div>

      {/* Open positions */}
      <Card title={`Open Positions (${sortedPositions.length}${allPositions.length !== sortedPositions.length ? `/${allPositions.length}` : ''})`}>
        {allPositions.length > 0 ? (
          <div>
            {/* Search filter */}
            <div className="mb-2">
              <input
                type="text"
                placeholder="Filter by market name…"
                value={search}
                onChange={e => setSearch(e.target.value)}
                className="w-full bg-gray-800 border border-gray-700 rounded px-3 py-1.5 text-xs text-white placeholder-gray-600 focus:outline-none focus:border-gray-500"
              />
            </div>
            <div className="overflow-x-auto">
              <table className="w-full text-xs">
                <thead>
                  <tr className="text-gray-500 border-b border-gray-800">
                    <th className="text-left py-2 px-2 cursor-pointer hover:text-gray-300 select-none" onClick={() => toggleSort('id')}>ID{sortIcon('id')}</th>
                    <th className="text-left py-2 px-2 cursor-pointer hover:text-gray-300 select-none" onClick={() => toggleSort('market_title')}>Market / Bet{sortIcon('market_title')}</th>
                    <th className="text-left py-2 px-2">Side</th>
                    <th className="text-right py-2 px-2 cursor-pointer hover:text-gray-300 select-none" onClick={() => toggleSort('entry_price')}>Entry{sortIcon('entry_price')}</th>
                    <th className="text-right py-2 px-2 cursor-pointer hover:text-gray-300 select-none" onClick={() => toggleSort('current_price')}>Current{sortIcon('current_price')}</th>
                    <th className="text-right py-2 px-2 cursor-pointer hover:text-gray-300 select-none" onClick={() => toggleSort('shares')}>Shares{sortIcon('shares')}</th>
                    <th className="text-right py-2 px-2 cursor-pointer hover:text-gray-300 select-none" onClick={() => toggleSort('opened_age_secs')}>Age{sortIcon('opened_age_secs')}</th>
                    <th className="text-right py-2 px-2 cursor-pointer hover:text-gray-300 select-none" onClick={() => toggleSort('usdc_spent')}>USDC{sortIcon('usdc_spent')}</th>
                    <th className="text-right py-2 px-2 cursor-pointer hover:text-gray-300 select-none" onClick={() => toggleSort('unrealized_pnl')}>P&L{sortIcon('unrealized_pnl')}</th>
                    <th className="text-right py-2 px-2 cursor-pointer hover:text-gray-300 select-none" onClick={() => toggleSort('unrealized_pnl_pct')}>P&L%{sortIcon('unrealized_pnl_pct')}</th>
                    <th className="text-right py-2 px-2">To Win</th>
                    <th className="py-2 px-2"></th>
                  </tr>
                </thead>
                <tbody>
                  {sortedPositions.map((pos) => {
                    const p_entry = pos.entry_price as number;
                    const p_current = pos.current_price as number;
                    const p_shares = pos.shares as number;
                    const p_pnl = pos.unrealized_pnl as number;
                    const p_pnlpct = pos.unrealized_pnl_pct as number;
                    const p_spent = pos.usdc_spent as number;
                    const p_age = pos.opened_age_secs as number;
                    const p_side = pos.side as string;
                    const p_id = pos.id as number;
                    return (
                    <tr key={p_id} className="border-b border-gray-800/50 hover:bg-gray-800/30">
                      <td className="py-1.5 px-2 text-gray-400">#{p_id}</td>
                      <td className="py-1.5 px-2">
                        <div className="text-white">{(pos.market_title as string) || (pos.token_id as string).slice(0, 12) + '...'}</div>
                        {(pos.market_outcome as string | undefined) && (
                          <div className="text-[10px] text-cyan-400">{pos.market_outcome as string}</div>
                        )}
                      </td>
                      <td className="py-1.5 px-2">
                        <Badge text={p_side} variant={p_side === 'BUY' ? 'green' : 'red'} />
                      </td>
                      <td className="py-1.5 px-2 text-right">{p_entry.toFixed(3)}</td>
                      <td className="py-1.5 px-2 text-right">{p_current.toFixed(3)}</td>
                      <td className="py-1.5 px-2 text-right">{p_shares.toFixed(1)}</td>
                      <td className="py-1.5 px-2 text-right">{formatAge(p_age)}</td>
                      <td className="py-1.5 px-2 text-right">${p_spent.toFixed(2)}</td>
                      <td className={`py-1.5 px-2 text-right ${p_pnl >= 0 ? 'text-emerald-400' : 'text-red-400'}`}>
                        {p_pnl >= 0 ? '+' : ''}${p_pnl.toFixed(2)}
                      </td>
                      <td className={`py-1.5 px-2 text-right ${p_pnlpct >= 0 ? 'text-emerald-400' : 'text-red-400'}`}>
                        {p_pnlpct >= 0 ? '+' : ''}{p_pnlpct.toFixed(1)}%
                      </td>
                      <td className="py-1.5 px-2 text-right text-cyan-400">
                        ${p_side === 'BUY'
                          ? p_shares.toFixed(2)
                          : (p_entry * p_shares + p_spent).toFixed(2)}
                      </td>
                      <td className="py-1.5 px-2">
                        <button
                          onClick={() => setSellPos(pos)}
                          className="px-2 py-0.5 rounded text-xs font-semibold border border-red-700 text-red-400 hover:bg-red-900/40 transition-colors"
                        >
                          Sell
                        </button>
                      </td>
                    </tr>
                    );
                  })}
                  {sortedPositions.length === 0 && (
                    <tr><td colSpan={11} className="py-3 text-center text-gray-600">No matching positions</td></tr>
                  )}
                </tbody>
              </table>
            </div>
          </div>
        ) : (
          <div className="text-gray-600 text-sm py-4">No open positions</div>
        )}
      </Card>

      {/* Stats bar */}
      <div className="grid grid-cols-2 md:grid-cols-5 gap-3">
        <Card><Stat label="Total Signals" value={n('total_signals')} /></Card>
        <Card><Stat label="Filled" value={n('filled_orders')} color="text-emerald-400" /></Card>
        <Card><Stat label="Skipped" value={n('skipped_orders')} color="text-yellow-400" /></Card>
        <Card><Stat label="Aborted" value={n('aborted_orders')} color="text-red-400" /></Card>
        <Card><Stat label="Avg Slippage" value={`${n('avg_slippage_bps').toFixed(1)} bps`} /></Card>
      </div>

      {sellPos && (
        <SellModal
          pos={sellPos}
          onClose={() => setSellPos(null)}
          onSold={() => setSellPos(null)}
        />
      )}
    </div>
  );
}
