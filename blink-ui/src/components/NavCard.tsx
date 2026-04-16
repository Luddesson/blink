import { useState, useEffect } from 'react'
import { TrendingUp, TrendingDown } from 'lucide-react'
import {
  Area,
  AreaChart,
  ResponsiveContainer,
  Tooltip,
  YAxis,
  XAxis,
  ReferenceLine,
} from 'recharts'
import { fmt, fmtPct, fmtPnl, pnlClass, fmtChartTime } from '../lib/format'
import { useMode } from '../hooks/useMode'
import type { PortfolioSummary } from '../types'

interface Props {
  nav: number
  navDelta: number
  navDeltaPct: number
  netPnl: number
  feesPaid: number
  equityCurve: number[]
  equityTimestamps: number[]
  portfolio?: PortfolioSummary
}

type TimeRange = '30m' | '1h' | '6h' | '24h'
const RANGES: TimeRange[] = ['30m', '1h', '6h', '24h']

const WINDOW_MS: Record<TimeRange, number> = {
  '30m': 30 * 60 * 1000,
  '1h':  60 * 60 * 1000,
  '6h':  6 * 60 * 60 * 1000,
  '24h': 24 * 60 * 60 * 1000,
}

interface FetchedPoint { timestamp_ms: number; nav_usdc: number }

function Stat({ label, value, muted }: { label: string; value: string; muted?: boolean }) {
  return (
    <div className="text-center">
      <div className="text-[10px] text-slate-600 uppercase tracking-wide mb-0.5">{label}</div>
      <div className={`text-sm font-mono font-semibold tabular-nums ${muted ? 'text-slate-600' : 'text-slate-200'}`}>
        {value}
      </div>
    </div>
  )
}

export default function NavCard({
  nav,
  navDelta,
  navDeltaPct,
  netPnl,
  feesPaid,
  equityCurve,
  equityTimestamps,
  portfolio,
}: Props) {
  const { viewMode } = useMode()
  const isLive = viewMode === 'live'
  const positive = netPnl >= 0

  // Client-side uptime ticker — uses authoritative engine_uptime_secs from WS
  // snapshot, with 1s client-side interpolation between updates.
  const [localUptime, setLocalUptime] = useState(portfolio?.uptime_secs ?? 0)
  const [lastUptimeSync, setLastUptimeSync] = useState(Date.now())
  useEffect(() => {
    if (portfolio?.uptime_secs != null) {
      setLocalUptime(portfolio.uptime_secs)
      setLastUptimeSync(Date.now())
    }
  }, [portfolio?.uptime_secs])
  useEffect(() => {
    const id = setInterval(() => {
      // Interpolate: server uptime + seconds since last WS update
      setLocalUptime(prev => {
        const serverBase = portfolio?.uptime_secs ?? prev
        const elapsed = Math.floor((Date.now() - lastUptimeSync) / 1000)
        return serverBase + elapsed
      })
    }, 1000)
    return () => clearInterval(id)
  }, [portfolio?.uptime_secs, lastUptimeSync])

  // nowMs ticks every second so the chart window scrolls in real-time
  const [nowMs, setNowMs] = useState(Date.now())
  useEffect(() => {
    const id = setInterval(() => setNowMs(Date.now()), 1000)
    return () => clearInterval(id)
  }, [])

  // Time-range selector state
  const [range, setRange] = useState<TimeRange>('30m')
  const [fetchedPoints, setFetchedPoints] = useState<FetchedPoint[]>([])

  // Fetch from /api/analytics/equity when range changes (or every 30s)
  useEffect(() => {
    let cancelled = false
    const load = async () => {
      try {
        const r = await fetch(`http://127.0.0.1:3030/api/analytics/equity?range=${range}`)
        if (!r.ok) return
        const data = await r.json() as { points: FetchedPoint[] }
        if (!cancelled && Array.isArray(data.points)) setFetchedPoints(data.points)
      } catch { /* engine may not be running */ }
    }
    void load()
    const id = setInterval(load, 30_000)
    return () => { cancelled = true; clearInterval(id) }
  }, [range])

  const strokeColor = positive
    ? (isLive ? '#f59e0b' : '#818cf8')
    : '#ef4444'

  // Prefer fetched historical data when available; fall back to in-memory WS curve
  const START_BALANCE = 100
  const rawChartData: { t: number; v: number }[] = fetchedPoints.length > 0
    ? fetchedPoints.map(p => ({ t: p.timestamp_ms, v: p.nav_usdc - START_BALANCE }))
    : equityCurve.map((v, i) => ({ t: equityTimestamps[i] ?? i, v: v - START_BALANCE }))

  // Chart window: right edge = now, left = now - selected range
  const chartWindowMs = WINDOW_MS[range]
  const windowStart = nowMs - chartWindowMs

  // Filter data to the visible window and inject a "now" anchor point
  const filteredData = rawChartData.filter(d => d.t >= windowStart && d.t <= nowMs)
  const currentPnl = nav - START_BALANCE
  // Append a synthetic "now" point so the chart always extends to the right edge
  if (filteredData.length > 0) {
    const last = filteredData[filteredData.length - 1]
    if (nowMs - last.t > 2000) {
      filteredData.push({ t: nowMs, v: currentPnl })
    }
  } else {
    // No data in window — show a flat line at current NAV
    filteredData.push({ t: windowStart, v: currentPnl })
    filteredData.push({ t: nowMs, v: currentPnl })
  }
  const chartData = filteredData

  // Generate tick marks — 6 evenly-spaced ticks
  const TICK_COUNT = 6
  const tickInterval = chartWindowMs / TICK_COUNT
  const xTicks: number[] = Array.from({ length: TICK_COUNT + 1 }, (_, i) =>
    windowStart + i * tickInterval
  )

  const fmtTickTime = (ms: number) =>
    new Date(ms).toLocaleTimeString('sv-SE', {
      timeZone: 'Europe/Stockholm',
      hour: '2-digit',
      minute: '2-digit',
    })

  const uptimeStr = localUptime < 3600
    ? `${Math.floor(localUptime / 60)}m ${localUptime % 60}s`
    : `${Math.floor(localUptime / 3600)}h ${Math.floor((localUptime % 3600) / 60)}m`

  return (
    <div className={`card ${isLive ? 'border-amber-900/60' : 'border-indigo-900/40'}`}>
      {/* Top row */}
      <div className="flex items-center justify-between mb-2">
        <span className="text-xs font-semibold uppercase tracking-widest text-slate-500">
          PnL Since Start
        </span>
        <div className="flex items-center gap-2">
          {portfolio && (
            <span className="text-xs text-slate-600 font-mono">uptime {uptimeStr}</span>
          )}
          <span className={`badge ${isLive ? 'badge-live' : 'badge-paper'}`}>
            {isLive ? 'LIVE' : 'PAPER'}
          </span>
        </div>
      </div>

      {/* Big PnL number */}
      <div className="flex items-baseline gap-3 flex-wrap">
        <span className={`text-4xl font-bold font-mono tabular-nums ${pnlClass(netPnl)}`}>
          {fmtPnl(netPnl)}
        </span>
        <span className={`text-base font-mono flex items-center gap-1 ${pnlClass(navDelta)}`}>
          {positive ? <TrendingUp size={14} /> : <TrendingDown size={14} />}
          {fmtPct(navDeltaPct)}
        </span>
        <span className="text-sm text-slate-500 font-mono">
          NAV <span className="text-slate-300">${fmt(nav)}</span>
        </span>
        {feesPaid > 0 && (
          <span className="text-xs text-amber-600 font-mono">fees −${fmt(feesPaid)}</span>
        )}
      </div>

      {/* Time-range selector */}
      <div className="flex items-center gap-1 mt-2">
        {RANGES.map(r => (
          <button
            key={r}
            onClick={() => setRange(r)}
            className={`text-[10px] px-2 py-0.5 rounded font-mono transition-colors ${
              range === r
                ? 'bg-indigo-800 text-indigo-200'
                : 'text-slate-600 hover:text-slate-400'
            }`}
          >
            {r}
          </button>
        ))}
        {fetchedPoints.length > 0 && (
          <span className="text-[9px] text-slate-700 ml-1">historical</span>
        )}
      </div>

      {/* Equity chart */}
      {chartData.length >= 1 ? (
        <div className="h-48 -mx-1 mt-2">
          <ResponsiveContainer width="100%" height="100%">
            <AreaChart data={chartData} margin={{ top: 4, right: 8, bottom: 18, left: 44 }}>
              <defs>
                <linearGradient id="navGrad" x1="0" y1="0" x2="0" y2="1">
                  <stop offset="0%" stopColor={strokeColor} stopOpacity={0.28} />
                  <stop offset="100%" stopColor={strokeColor} stopOpacity={0.01} />
                </linearGradient>
              </defs>
              <YAxis
                domain={['dataMin - 0.05', 'dataMax + 0.05']}
                tickFormatter={(v: number) => (v >= 0 ? `+${v.toFixed(2)}` : v.toFixed(2))}
                tick={{ fill: '#475569', fontSize: 9 }}
                width={42}
              />
              <XAxis
                dataKey="t"
                type="number"
                scale="time"
                domain={[windowStart, nowMs]}
                allowDataOverflow={true}
                ticks={xTicks}
                tickFormatter={fmtTickTime}
                tick={{ fill: '#475569', fontSize: 9 }}
                height={18}
              />
              {/* Breakeven line */}
              <ReferenceLine y={0} stroke="#334155" strokeDasharray="4 3" />
              {/* Subtle grid lines */}
              {chartData.length > 1 && (() => {
                const vals = chartData.map(d => d.v)
                const mn = Math.min(...vals)
                const mx = Math.max(...vals)
                const rng = mx - mn || 1
                return [0.2, 0.4, 0.6, 0.8].map(p => (
                  <ReferenceLine
                    key={p}
                    y={mn + rng * p}
                    stroke="#1e293b"
                    strokeDasharray="2 4"
                    strokeOpacity={0.5}
                  />
                ))
              })()}
              <Tooltip
                contentStyle={{
                  background: '#0f172a',
                  border: '1px solid #334155',
                  borderRadius: 6,
                  fontSize: 11,
                }}
                labelFormatter={(label) => fmtChartTime(Number(label))}
                formatter={(value) => {
                  const v = Number(value ?? 0)
                  return [
                  `${v >= 0 ? '+' : ''}$${Math.abs(v).toFixed(2)}`,
                  'P&L',
                  ]
                }}
              />
              <Area
                type="monotone"
                dataKey="v"
                stroke={strokeColor}
                strokeWidth={2}
                fill="url(#navGrad)"
                dot={false}
                isAnimationActive={false}
              />
            </AreaChart>
          </ResponsiveContainer>
        </div>
      ) : (
        <div className="h-48 flex items-center justify-center text-slate-700 text-sm mt-2 border border-dashed border-surface-700 rounded-lg">
          Waiting for equity data…
        </div>
      )}

      {/* Stats strip */}
      {portfolio && (
        <div className="grid grid-cols-6 gap-1 mt-3 pt-3 border-t border-surface-700">
          <Stat label="Cash" value={`$${fmt(portfolio.cash_usdc, 0)}`} />
          <Stat label="Invested" value={`$${fmt(portfolio.invested_usdc, 0)}`} />
          <Stat label="Fees" value={feesPaid > 0 ? `-$${fmt(feesPaid)}` : '$0.00'} muted={feesPaid === 0} />
          <Stat label="Fill %" value={`${fmt(portfolio.fill_rate_pct, 1)}%`} />
          <Stat label="Win %" value={`${fmt(portfolio.win_rate_pct, 1)}%`} />
          <Stat label="Trades" value={String(portfolio.closed_trades_count ?? 0)} />
        </div>
      )}
    </div>
  )
}