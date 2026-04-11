import { useState } from 'react'
import { fmt } from '../lib/format'
import { LineChart, Line, XAxis, YAxis, Tooltip, ResponsiveContainer, ReferenceLine } from 'recharts'

// ─── Types ────────────────────────────────────────────────────────────────────

interface BacktestConfig {
  rn1_wallet: string
  tick_path: string
  starting_usdc: number
  size_multiplier: number
  drift_threshold: number
  fill_window_ms: number
  slippage_bps: number
}

interface BacktestResult {
  ok: boolean
  error?: string
  tick_count?: number
  total_return_pct?: number
  sharpe_ratio?: number
  sortino_ratio?: number
  max_drawdown_pct?: number
  calmar_ratio?: number
  win_rate?: number
  profit_factor?: number
  avg_trade_duration_ms?: number
  total_trades?: number
  equity_curve?: [number, number][]
}

interface SweepRow {
  size_multiplier: number
  slippage_bps: number
  drift_threshold: number
  fill_window_ms: number
  total_return_pct: number
  sharpe_ratio: number
  max_drawdown_pct: number
  win_rate: number
  profit_factor: number
  total_trades: number
}

interface SweepResult {
  ok: boolean
  error?: string
  tick_count?: number
  combinations_run?: number
  results?: SweepRow[]
}

interface WalkWindow {
  window: number
  tick_count: number
  start_ms: number
  end_ms: number
  total_return_pct: number
  sharpe_ratio: number
  sortino_ratio: number
  max_drawdown_pct: number
  win_rate: number
  profit_factor: number
  total_trades: number
}

interface WalkForwardResult {
  ok: boolean
  error?: string
  tick_count?: number
  num_windows?: number
  windows?: WalkWindow[]
  aggregate?: {
    avg_return_pct: number
    avg_sharpe: number
    avg_max_drawdown_pct: number
    avg_win_rate: number
    pct_profitable_windows: number
    consistency_score: number
  }
}

// ─── Defaults ─────────────────────────────────────────────────────────────────

const DEFAULT_CONFIG: BacktestConfig = {
  rn1_wallet: '',
  tick_path: 'logs/ticks.csv',
  starting_usdc: 100,
  size_multiplier: 0.05,
  drift_threshold: 0.015,
  fill_window_ms: 3000,
  slippage_bps: 10,
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

function fmtDurMs(ms: number): string {
  if (ms < 1000) return `${ms}ms`
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)}s`
  return `${(ms / 60_000).toFixed(1)}m`
}

function pct(v: number | undefined): string {
  return `${fmt(v ?? 0, 2)}%`
}

function colorForReturn(v: number) {
  return v >= 0 ? 'text-emerald-400' : 'text-rose-400'
}

function colorForSharpe(v: number) {
  return v >= 1 ? 'text-emerald-400' : v >= 0 ? 'text-amber-400' : 'text-rose-400'
}

function MetricBox({ label, value, sub, color = 'text-slate-200' }: { label: string; value: string; sub?: string; color?: string }) {
  return (
    <div className="bg-surface-900 rounded-lg px-3 py-2">
      <div className="text-[10px] text-slate-500 uppercase tracking-wider mb-0.5">{label}</div>
      <div className={`font-mono font-bold text-lg ${color}`}>{value}</div>
      {sub && <div className="text-[10px] text-slate-600 mt-0.5">{sub}</div>}
    </div>
  )
}

function SharedFields({ config, update }: { config: BacktestConfig; update: (f: keyof BacktestConfig, v: string) => void }) {
  return (
    <>
      <label className="flex flex-col gap-1">
        <span className="text-slate-400">RN1 Wallet (optional)</span>
        <input
          className="bg-surface-900 border border-slate-700 rounded px-2 py-1.5 font-mono text-slate-200 placeholder-slate-600 focus:outline-none focus:border-slate-500"
          placeholder="0x… (uses env default)"
          value={config.rn1_wallet}
          onChange={(e) => update('rn1_wallet', e.target.value)}
        />
      </label>
      <label className="flex flex-col gap-1">
        <span className="text-slate-400">Tick CSV Path</span>
        <input
          className="bg-surface-900 border border-slate-700 rounded px-2 py-1.5 font-mono text-slate-200 focus:outline-none focus:border-slate-500"
          value={config.tick_path}
          onChange={(e) => update('tick_path', e.target.value)}
        />
      </label>
      <label className="flex flex-col gap-1">
        <span className="text-slate-400">Starting USDC</span>
        <input type="number" min={1} step={10}
          className="bg-surface-900 border border-slate-700 rounded px-2 py-1.5 font-mono text-slate-200 focus:outline-none focus:border-slate-500"
          value={config.starting_usdc} onChange={(e) => update('starting_usdc', e.target.value)} />
      </label>
    </>
  )
}

type SubTab = 'single' | 'sweep' | 'walk-forward'

// ─── Main component ───────────────────────────────────────────────────────────

export default function BacktestPage() {
  const [subTab, setSubTab] = useState<SubTab>('single')
  const [config, setConfig] = useState<BacktestConfig>(DEFAULT_CONFIG)
  const [running, setRunning] = useState(false)

  // Single run state
  const [result, setResult] = useState<BacktestResult | null>(null)

  // Sweep state
  const [sweepResult, setSweepResult] = useState<SweepResult | null>(null)
  const [sweepAxes, setSweepAxes] = useState({
    size_multiplier: '0.02,0.05,0.08',
    slippage_bps: '5,10,20',
    drift_threshold: '0.01,0.015,0.02',
    fill_window_ms: '',
  })

  // Walk-forward state
  const [wfResult, setWfResult] = useState<WalkForwardResult | null>(null)
  const [numWindows, setNumWindows] = useState(5)

  function update(field: keyof BacktestConfig, raw: string) {
    const numFields: (keyof BacktestConfig)[] = [
      'starting_usdc', 'size_multiplier', 'drift_threshold', 'fill_window_ms', 'slippage_bps',
    ]
    setConfig((c) => ({
      ...c,
      [field]: numFields.includes(field) ? parseFloat(raw) || 0 : raw,
    }))
  }

  function parseAxis(raw: string): number[] {
    return raw.split(',').map((s) => parseFloat(s.trim())).filter((v) => !isNaN(v))
  }

  async function runSingle() {
    setRunning(true); setResult(null)
    try {
      const res = await fetch('/api/backtest', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          rn1_wallet: config.rn1_wallet || undefined,
          tick_path: config.tick_path || undefined,
          starting_usdc: config.starting_usdc,
          size_multiplier: config.size_multiplier,
          drift_threshold: config.drift_threshold,
          fill_window_ms: Math.round(config.fill_window_ms),
          slippage_bps: Math.round(config.slippage_bps),
        }),
      })
      setResult(await res.json() as BacktestResult)
    } catch (e) { setResult({ ok: false, error: String(e) }) }
    finally { setRunning(false) }
  }

  async function runSweep() {
    setRunning(true); setSweepResult(null)
    const smVals = parseAxis(sweepAxes.size_multiplier)
    const sbVals = parseAxis(sweepAxes.slippage_bps).map(Math.round)
    const dtVals = parseAxis(sweepAxes.drift_threshold)
    const fwVals = parseAxis(sweepAxes.fill_window_ms).map(Math.round)
    try {
      const res = await fetch('/api/backtest/sweep', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          rn1_wallet: config.rn1_wallet || undefined,
          tick_path: config.tick_path || undefined,
          starting_usdc: config.starting_usdc,
          sweep: {
            size_multiplier: smVals.length ? smVals : undefined,
            slippage_bps: sbVals.length ? sbVals : undefined,
            drift_threshold: dtVals.length ? dtVals : undefined,
            fill_window_ms: fwVals.length ? fwVals : undefined,
          },
        }),
      })
      setSweepResult(await res.json() as SweepResult)
    } catch (e) { setSweepResult({ ok: false, error: String(e) }) }
    finally { setRunning(false) }
  }

  async function runWalkForward() {
    setRunning(true); setWfResult(null)
    try {
      const res = await fetch('/api/backtest/walk-forward', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          rn1_wallet: config.rn1_wallet || undefined,
          tick_path: config.tick_path || undefined,
          starting_usdc: config.starting_usdc,
          size_multiplier: config.size_multiplier,
          drift_threshold: config.drift_threshold,
          fill_window_ms: Math.round(config.fill_window_ms),
          slippage_bps: Math.round(config.slippage_bps),
          num_windows: numWindows,
        }),
      })
      setWfResult(await res.json() as WalkForwardResult)
    } catch (e) { setWfResult({ ok: false, error: String(e) }) }
    finally { setRunning(false) }
  }

  const chartData = result?.equity_curve?.map(([ts, nav]) => ({ ts, nav })) ?? []

  return (
    <div className="flex-1 flex flex-col gap-3 p-3 overflow-y-auto min-h-0">
      {/* ── Mode selector ───────────────────────────────────────────────── */}
      <div className="flex gap-1 bg-surface-900 rounded-lg p-1 w-fit">
        {(['single', 'sweep', 'walk-forward'] as SubTab[]).map((t) => (
          <button
            key={t}
            onClick={() => setSubTab(t)}
            className={`px-3 py-1 rounded text-xs font-medium transition-colors ${
              subTab === t ? 'bg-indigo-600 text-white' : 'text-slate-400 hover:text-slate-200'
            }`}
          >
            {t === 'single' ? '▶ Single Run' : t === 'sweep' ? '⚡ Parameter Sweep' : '📅 Walk-Forward'}
          </button>
        ))}
      </div>

      {/* ── Single Run ──────────────────────────────────────────────────── */}
      {subTab === 'single' && (
        <>
          <div className="card">
            <span className="text-xs font-semibold uppercase tracking-widest text-slate-500 mb-3 block">
              Backtest Configuration
            </span>
            <div className="grid grid-cols-2 md:grid-cols-4 gap-3 text-xs">
              <SharedFields config={config} update={update} />
              <label className="flex flex-col gap-1">
                <span className="text-slate-400">Size Multiplier</span>
                <input type="number" min={0.001} max={1} step={0.005}
                  className="bg-surface-900 border border-slate-700 rounded px-2 py-1.5 font-mono text-slate-200 focus:outline-none focus:border-slate-500"
                  value={config.size_multiplier} onChange={(e) => update('size_multiplier', e.target.value)} />
              </label>
              <label className="flex flex-col gap-1">
                <span className="text-slate-400">Drift Threshold</span>
                <input type="number" min={0.001} max={0.1} step={0.005}
                  className="bg-surface-900 border border-slate-700 rounded px-2 py-1.5 font-mono text-slate-200 focus:outline-none focus:border-slate-500"
                  value={config.drift_threshold} onChange={(e) => update('drift_threshold', e.target.value)} />
              </label>
              <label className="flex flex-col gap-1">
                <span className="text-slate-400">Fill Window (ms)</span>
                <input type="number" min={100} max={30000} step={500}
                  className="bg-surface-900 border border-slate-700 rounded px-2 py-1.5 font-mono text-slate-200 focus:outline-none focus:border-slate-500"
                  value={config.fill_window_ms} onChange={(e) => update('fill_window_ms', e.target.value)} />
              </label>
              <label className="flex flex-col gap-1">
                <span className="text-slate-400">Slippage (bps)</span>
                <input type="number" min={0} max={200} step={5}
                  className="bg-surface-900 border border-slate-700 rounded px-2 py-1.5 font-mono text-slate-200 focus:outline-none focus:border-slate-500"
                  value={config.slippage_bps} onChange={(e) => update('slippage_bps', e.target.value)} />
              </label>
              <div className="flex items-end">
                <button onClick={runSingle} disabled={running}
                  className="w-full px-4 py-2 rounded bg-indigo-600 hover:bg-indigo-500 disabled:opacity-50 disabled:cursor-not-allowed text-white text-xs font-semibold transition-colors">
                  {running ? '⏳ Running…' : '▶ Run'}
                </button>
              </div>
            </div>
          </div>

          {result && !result.ok && (
            <div className="card border border-rose-700/50">
              <p className="text-xs text-rose-400"><span className="font-semibold">Error: </span>{result.error}</p>
            </div>
          )}

          {result?.ok && (
            <>
              <div className="card">
                <span className="text-xs font-semibold uppercase tracking-widest text-slate-500 mb-3 block">
                  Results — {result.tick_count?.toLocaleString()} ticks · {result.total_trades} trades
                </span>
                <div className="grid grid-cols-4 gap-3">
                  <MetricBox label="Total Return" value={pct(result.total_return_pct)}
                    color={colorForReturn(result.total_return_pct ?? 0)} />
                  <MetricBox label="Sharpe" value={fmt(result.sharpe_ratio ?? 0, 3)}
                    color={colorForSharpe(result.sharpe_ratio ?? 0)} />
                  <MetricBox label="Sortino" value={fmt(result.sortino_ratio ?? 0, 3)}
                    color={(result.sortino_ratio ?? 0) >= 1 ? 'text-emerald-400' : 'text-slate-300'} />
                  <MetricBox label="Max Drawdown" value={pct(result.max_drawdown_pct)}
                    color={(result.max_drawdown_pct ?? 0) > 15 ? 'text-rose-400' : 'text-amber-400'} />
                  <MetricBox label="Win Rate" value={pct((result.win_rate ?? 0) * 100)}
                    sub={`${result.total_trades} trades`}
                    color={(result.win_rate ?? 0) >= 0.55 ? 'text-emerald-400' : 'text-slate-300'} />
                  <MetricBox label="Profit Factor"
                    value={isFinite(result.profit_factor ?? 0) ? fmt(result.profit_factor ?? 0, 2) : '∞'}
                    color={(result.profit_factor ?? 0) >= 1.5 ? 'text-emerald-400' : 'text-slate-300'} />
                  <MetricBox label="Calmar"
                    value={isFinite(result.calmar_ratio ?? 0) ? fmt(result.calmar_ratio ?? 0, 2) : '∞'}
                    color={(result.calmar_ratio ?? 0) >= 1 ? 'text-emerald-400' : 'text-slate-300'} />
                  <MetricBox label="Avg Trade Duration" value={fmtDurMs(result.avg_trade_duration_ms ?? 0)} />
                </div>
              </div>

              {chartData.length > 1 && (
                <div className="card">
                  <span className="text-xs font-semibold uppercase tracking-widest text-slate-500 mb-3 block">
                    Equity Curve
                  </span>
                  <ResponsiveContainer width="100%" height={200}>
                    <LineChart data={chartData} margin={{ top: 4, right: 8, left: 0, bottom: 4 }}>
                      <XAxis dataKey="ts" hide />
                      <YAxis domain={['auto', 'auto']}
                        tickFormatter={(v: number) => `$${v.toFixed(0)}`}
                        tick={{ fontSize: 10, fill: '#64748b' }} width={48} />
                      <Tooltip contentStyle={{ background: '#1e293b', border: '1px solid #334155', borderRadius: 6 }}
                        labelStyle={{ color: '#94a3b8', fontSize: 10 }}
                        formatter={(v: number) => [`$${v.toFixed(2)}`, 'NAV']}
                        labelFormatter={() => ''} />
                      <ReferenceLine y={config.starting_usdc} stroke="#475569" strokeDasharray="3 3" />
                      <Line type="monotone" dataKey="nav" stroke="#818cf8" strokeWidth={1.5}
                        dot={false} isAnimationActive={false} />
                    </LineChart>
                  </ResponsiveContainer>
                </div>
              )}
            </>
          )}
        </>
      )}

      {/* ── Parameter Sweep ─────────────────────────────────────────────── */}
      {subTab === 'sweep' && (
        <>
          <div className="card">
            <span className="text-xs font-semibold uppercase tracking-widest text-slate-500 mb-3 block">
              Sweep Configuration — enter comma-separated values to test
            </span>
            <div className="grid grid-cols-2 md:grid-cols-4 gap-3 text-xs">
              <SharedFields config={config} update={update} />
              {([
                ['size_multiplier', 'Size Multiplier values'],
                ['slippage_bps', 'Slippage bps values'],
                ['drift_threshold', 'Drift Threshold values'],
                ['fill_window_ms', 'Fill Window (ms) values'],
              ] as [keyof typeof sweepAxes, string][]).map(([field, label]) => (
                <label key={field} className="flex flex-col gap-1">
                  <span className="text-slate-400">{label}</span>
                  <input
                    className="bg-surface-900 border border-slate-700 rounded px-2 py-1.5 font-mono text-slate-200 placeholder-slate-600 focus:outline-none focus:border-slate-500"
                    placeholder="e.g. 0.02,0.05,0.08"
                    value={sweepAxes[field]}
                    onChange={(e) => setSweepAxes((a) => ({ ...a, [field]: e.target.value }))}
                  />
                </label>
              ))}
              <div className="flex items-end">
                <button onClick={runSweep} disabled={running}
                  className="w-full px-4 py-2 rounded bg-violet-600 hover:bg-violet-500 disabled:opacity-50 disabled:cursor-not-allowed text-white text-xs font-semibold transition-colors">
                  {running ? '⏳ Running…' : '⚡ Run Sweep'}
                </button>
              </div>
            </div>
          </div>

          {sweepResult && !sweepResult.ok && (
            <div className="card border border-rose-700/50">
              <p className="text-xs text-rose-400"><span className="font-semibold">Error: </span>{sweepResult.error}</p>
            </div>
          )}

          {sweepResult?.ok && sweepResult.results && (
            <div className="card">
              <span className="text-xs font-semibold uppercase tracking-widest text-slate-500 mb-3 block">
                Sweep Results — {sweepResult.combinations_run} combos · {sweepResult.tick_count?.toLocaleString()} ticks · ranked by Sharpe
              </span>
              <div className="overflow-x-auto">
                <table className="w-full text-[11px] text-slate-300">
                  <thead>
                    <tr className="text-[10px] text-slate-500 uppercase border-b border-slate-800">
                      <th className="text-left pb-1.5 pr-3">#</th>
                      <th className="text-right pb-1.5 pr-3">Size×</th>
                      <th className="text-right pb-1.5 pr-3">Slip bps</th>
                      <th className="text-right pb-1.5 pr-3">Drift%</th>
                      <th className="text-right pb-1.5 pr-3">FillWin ms</th>
                      <th className="text-right pb-1.5 pr-3">Return</th>
                      <th className="text-right pb-1.5 pr-3">Sharpe</th>
                      <th className="text-right pb-1.5 pr-3">MaxDD</th>
                      <th className="text-right pb-1.5 pr-3">WinRate</th>
                      <th className="text-right pb-1.5">Trades</th>
                    </tr>
                  </thead>
                  <tbody>
                    {sweepResult.results.slice(0, 30).map((row, i) => (
                      <tr key={i} className={`border-b border-slate-800/50 ${i === 0 ? 'bg-indigo-900/20' : ''}`}>
                        <td className="py-1 pr-3 text-slate-500">{i + 1}</td>
                        <td className="py-1 pr-3 text-right font-mono">{row.size_multiplier.toFixed(3)}</td>
                        <td className="py-1 pr-3 text-right font-mono">{row.slippage_bps}</td>
                        <td className="py-1 pr-3 text-right font-mono">{(row.drift_threshold * 100).toFixed(1)}%</td>
                        <td className="py-1 pr-3 text-right font-mono">{row.fill_window_ms}</td>
                        <td className={`py-1 pr-3 text-right font-mono ${colorForReturn(row.total_return_pct)}`}>
                          {pct(row.total_return_pct)}
                        </td>
                        <td className={`py-1 pr-3 text-right font-mono ${colorForSharpe(row.sharpe_ratio)}`}>
                          {fmt(row.sharpe_ratio, 3)}
                        </td>
                        <td className={`py-1 pr-3 text-right font-mono ${row.max_drawdown_pct > 15 ? 'text-rose-400' : 'text-slate-300'}`}>
                          {pct(row.max_drawdown_pct)}
                        </td>
                        <td className="py-1 pr-3 text-right font-mono">{pct(row.win_rate * 100)}</td>
                        <td className="py-1 text-right font-mono text-slate-400">{row.total_trades}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
                {sweepResult.results.length > 30 && (
                  <p className="text-[10px] text-slate-600 mt-1 text-right">
                    Showing top 30 of {sweepResult.results.length}
                  </p>
                )}
              </div>
            </div>
          )}
        </>
      )}

      {/* ── Walk-Forward ────────────────────────────────────────────────── */}
      {subTab === 'walk-forward' && (
        <>
          <div className="card">
            <span className="text-xs font-semibold uppercase tracking-widest text-slate-500 mb-3 block">
              Walk-Forward Configuration
            </span>
            <div className="grid grid-cols-2 md:grid-cols-4 gap-3 text-xs">
              <SharedFields config={config} update={update} />
              <label className="flex flex-col gap-1">
                <span className="text-slate-400">Size Multiplier</span>
                <input type="number" min={0.001} max={1} step={0.005}
                  className="bg-surface-900 border border-slate-700 rounded px-2 py-1.5 font-mono text-slate-200 focus:outline-none focus:border-slate-500"
                  value={config.size_multiplier} onChange={(e) => update('size_multiplier', e.target.value)} />
              </label>
              <label className="flex flex-col gap-1">
                <span className="text-slate-400">Slippage (bps)</span>
                <input type="number" min={0} max={200} step={5}
                  className="bg-surface-900 border border-slate-700 rounded px-2 py-1.5 font-mono text-slate-200 focus:outline-none focus:border-slate-500"
                  value={config.slippage_bps} onChange={(e) => update('slippage_bps', e.target.value)} />
              </label>
              <label className="flex flex-col gap-1">
                <span className="text-slate-400">Number of Windows</span>
                <input type="number" min={2} max={20} step={1}
                  className="bg-surface-900 border border-slate-700 rounded px-2 py-1.5 font-mono text-slate-200 focus:outline-none focus:border-slate-500"
                  value={numWindows} onChange={(e) => setNumWindows(parseInt(e.target.value) || 5)} />
              </label>
              <div className="flex items-end">
                <button onClick={runWalkForward} disabled={running}
                  className="w-full px-4 py-2 rounded bg-teal-600 hover:bg-teal-500 disabled:opacity-50 disabled:cursor-not-allowed text-white text-xs font-semibold transition-colors">
                  {running ? '⏳ Running…' : '📅 Run WF'}
                </button>
              </div>
            </div>
          </div>

          {wfResult && !wfResult.ok && (
            <div className="card border border-rose-700/50">
              <p className="text-xs text-rose-400"><span className="font-semibold">Error: </span>{wfResult.error}</p>
            </div>
          )}

          {wfResult?.ok && wfResult.aggregate && (
            <>
              <div className="card">
                <span className="text-xs font-semibold uppercase tracking-widest text-slate-500 mb-3 block">
                  Aggregate — {wfResult.num_windows} windows · {wfResult.tick_count?.toLocaleString()} ticks
                </span>
                <div className="grid grid-cols-3 md:grid-cols-6 gap-3">
                  <MetricBox label="Avg Return" value={pct(wfResult.aggregate.avg_return_pct)}
                    color={colorForReturn(wfResult.aggregate.avg_return_pct)} />
                  <MetricBox label="Avg Sharpe" value={fmt(wfResult.aggregate.avg_sharpe, 3)}
                    color={colorForSharpe(wfResult.aggregate.avg_sharpe)} />
                  <MetricBox label="Avg MaxDD" value={pct(wfResult.aggregate.avg_max_drawdown_pct)}
                    color={wfResult.aggregate.avg_max_drawdown_pct > 15 ? 'text-rose-400' : 'text-amber-400'} />
                  <MetricBox label="Avg Win Rate" value={pct(wfResult.aggregate.avg_win_rate * 100)}
                    color={wfResult.aggregate.avg_win_rate >= 0.55 ? 'text-emerald-400' : 'text-slate-300'} />
                  <MetricBox label="% Profitable" value={pct(wfResult.aggregate.pct_profitable_windows * 100)}
                    color={wfResult.aggregate.pct_profitable_windows >= 0.6 ? 'text-emerald-400' : 'text-slate-300'} />
                  <MetricBox label="Consistency" value={fmt(wfResult.aggregate.consistency_score, 3)}
                    color={wfResult.aggregate.consistency_score >= 0.5 ? 'text-emerald-400' : 'text-slate-300'} />
                </div>
              </div>

              {wfResult.windows && (
                <div className="card">
                  <span className="text-xs font-semibold uppercase tracking-widest text-slate-500 mb-3 block">
                    Per-Window Results
                  </span>
                  <div className="overflow-x-auto">
                    <table className="w-full text-[11px] text-slate-300">
                      <thead>
                        <tr className="text-[10px] text-slate-500 uppercase border-b border-slate-800">
                          <th className="text-left pb-1.5 pr-3">Window</th>
                          <th className="text-right pb-1.5 pr-3">Ticks</th>
                          <th className="text-right pb-1.5 pr-3">Return</th>
                          <th className="text-right pb-1.5 pr-3">Sharpe</th>
                          <th className="text-right pb-1.5 pr-3">Sortino</th>
                          <th className="text-right pb-1.5 pr-3">MaxDD</th>
                          <th className="text-right pb-1.5 pr-3">WinRate</th>
                          <th className="text-right pb-1.5">Trades</th>
                        </tr>
                      </thead>
                      <tbody>
                        {wfResult.windows.map((w) => (
                          <tr key={w.window} className="border-b border-slate-800/50">
                            <td className="py-1 pr-3 text-slate-400">W{w.window}</td>
                            <td className="py-1 pr-3 text-right font-mono text-slate-500">{w.tick_count.toLocaleString()}</td>
                            <td className={`py-1 pr-3 text-right font-mono ${colorForReturn(w.total_return_pct)}`}>
                              {pct(w.total_return_pct)}
                            </td>
                            <td className={`py-1 pr-3 text-right font-mono ${colorForSharpe(w.sharpe_ratio)}`}>
                              {fmt(w.sharpe_ratio, 3)}
                            </td>
                            <td className="py-1 pr-3 text-right font-mono text-slate-300">{fmt(w.sortino_ratio, 3)}</td>
                            <td className={`py-1 pr-3 text-right font-mono ${w.max_drawdown_pct > 15 ? 'text-rose-400' : 'text-slate-300'}`}>
                              {pct(w.max_drawdown_pct)}
                            </td>
                            <td className="py-1 pr-3 text-right font-mono">{pct(w.win_rate * 100)}</td>
                            <td className="py-1 text-right font-mono text-slate-400">{w.total_trades}</td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </div>
                </div>
              )}
            </>
          )}
        </>
      )}
    </div>
  )
}
