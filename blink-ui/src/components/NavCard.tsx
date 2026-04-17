import { useState, useEffect } from 'react'
import { TrendingUp, TrendingDown } from 'lucide-react'
import { motion } from 'motion/react'
import {
  Area,
  AreaChart,
  ResponsiveContainer,
  Tooltip,
  YAxis,
  XAxis,
  ReferenceLine,
} from 'recharts'
import { fmt, fmtPct, fmtChartTime } from '../lib/format'
import { useMode } from '../hooks/useMode'
import { Badge } from './ui'
import NumberFlip from './motion/NumberFlip'
import { cn } from '../lib/cn'
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
      <div className="text-[10px] uppercase tracking-[0.12em] text-[color:var(--color-text-muted)] mb-0.5">
        {label}
      </div>
      <div className={cn(
        'text-sm font-mono font-semibold tabular',
        muted ? 'text-[color:var(--color-text-dim)]' : 'text-[color:var(--color-text-primary)]',
      )}>
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
      setLocalUptime(prev => {
        const serverBase = portfolio?.uptime_secs ?? prev
        const elapsed = Math.floor((Date.now() - lastUptimeSync) / 1000)
        return serverBase + elapsed
      })
    }, 1000)
    return () => clearInterval(id)
  }, [portfolio?.uptime_secs, lastUptimeSync])

  const [nowMs, setNowMs] = useState(Date.now())
  useEffect(() => {
    const id = setInterval(() => setNowMs(Date.now()), 1000)
    return () => clearInterval(id)
  }, [])

  const [range, setRange] = useState<TimeRange>('30m')
  const [fetchedPoints, setFetchedPoints] = useState<FetchedPoint[]>([])

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

  // Stroke color: bull-emerald when profitable, bear-ruby when down.
  // In live mode, tint toward amber for positive moves.
  const strokeColor = positive
    ? (isLive ? 'oklch(0.78 0.18 65)' : 'oklch(0.78 0.18 155)')
    : 'oklch(0.72 0.22 25)'

  const START_BALANCE = 100
  const rawChartData: { t: number; v: number }[] = fetchedPoints.length > 0
    ? fetchedPoints.map(p => ({ t: p.timestamp_ms, v: p.nav_usdc - START_BALANCE }))
    : equityCurve.map((v, i) => ({ t: equityTimestamps[i] ?? i, v: v - START_BALANCE }))

  const chartWindowMs = WINDOW_MS[range]
  const windowStart = nowMs - chartWindowMs

  const filteredData = rawChartData.filter(d => d.t >= windowStart && d.t <= nowMs)
  const currentPnl = nav - START_BALANCE
  if (filteredData.length > 0) {
    const last = filteredData[filteredData.length - 1]
    if (nowMs - last.t > 2000) filteredData.push({ t: nowMs, v: currentPnl })
  } else {
    filteredData.push({ t: windowStart, v: currentPnl })
    filteredData.push({ t: nowMs, v: currentPnl })
  }
  const chartData = filteredData

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
    <motion.div
      initial={{ opacity: 0, y: 10 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.32, ease: [0.2, 0, 0, 1] }}
      className={cn(
        'relative rounded-xl p-5 overflow-hidden glass',
        isLive
          ? 'shadow-[0_0_0_1px_oklch(0.65_0.24_25/0.25),0_24px_64px_-20px_oklch(0.65_0.24_25/0.4)]'
          : 'shadow-[0_0_0_1px_oklch(0.75_0.18_170/0.22),0_24px_64px_-20px_oklch(0.70_0.22_290/0.35)]',
      )}
    >
      {/* Ambient aurora tint inside the card */}
      <div
        aria-hidden="true"
        className="absolute inset-0 opacity-40 pointer-events-none"
        style={{
          background: isLive
            ? 'radial-gradient(120% 80% at 85% 0%, oklch(0.65 0.24 25 / 0.2), transparent 55%)'
            : 'radial-gradient(120% 80% at 85% 0%, oklch(0.75 0.18 170 / 0.18), transparent 55%), radial-gradient(100% 60% at 15% 100%, oklch(0.70 0.22 290 / 0.15), transparent 55%)',
        }}
      />

      <div className="relative">
        {/* Top row */}
        <div className="flex items-center justify-between mb-3">
          <span className="serif-accent text-[15px] text-[color:var(--color-text-primary)]">
            PnL since start
          </span>
          <div className="flex items-center gap-2">
            {portfolio && (
              <span className="text-[10px] text-[color:var(--color-text-muted)] font-mono tabular">
                uptime {uptimeStr}
              </span>
            )}
            <Badge variant={isLive ? 'live' : 'paper'} dot>
              {isLive ? 'LIVE' : 'PAPER'}
            </Badge>
          </div>
        </div>

        {/* Big PnL number — animated flip */}
        <div className="flex items-baseline gap-3 flex-wrap">
          <NumberFlip
            value={netPnl}
            format={(v) => `${v >= 0 ? '+' : '−'}$${Math.abs(v).toFixed(2)}`}
            className={cn(
              'text-[44px] font-bold leading-none',
              positive
                ? 'text-[color:var(--color-bull-400)] drop-shadow-[0_0_28px_oklch(0.72_0.19_155/0.45)]'
                : 'text-[color:var(--color-bear-400)] drop-shadow-[0_0_28px_oklch(0.65_0.24_25/0.45)]',
            )}
          />
          <span
            className={cn(
              'text-base font-mono tabular flex items-center gap-1 font-semibold',
              navDelta >= 0 ? 'text-[color:var(--color-bull-400)]' : 'text-[color:var(--color-bear-400)]',
            )}
          >
            {positive ? <TrendingUp size={15} /> : <TrendingDown size={15} />}
            {fmtPct(navDeltaPct)}
          </span>
          <span className="text-sm text-[color:var(--color-text-muted)] font-mono tabular">
            NAV <span className="text-[color:var(--color-text-primary)] font-semibold">${fmt(nav)}</span>
          </span>
          {feesPaid > 0 && (
            <span className="text-xs text-[color:var(--color-whale-500)] font-mono tabular">
              fees −${fmt(feesPaid)}
            </span>
          )}
        </div>

        {/* Time-range selector */}
        <div className="flex items-center gap-1 mt-3">
          {RANGES.map(r => (
            <button
              key={r}
              onClick={() => setRange(r)}
              className={cn(
                'text-[10px] px-2.5 py-1 rounded-md font-mono tabular transition-colors relative',
                range === r
                  ? 'text-[color:var(--color-text-primary)] bg-[color:oklch(0.75_0.18_170/0.12)] border border-[color:oklch(0.75_0.18_170/0.4)]'
                  : 'text-[color:var(--color-text-muted)] hover:text-[color:var(--color-text-primary)] hover:bg-[color:oklch(0.26_0.022_260/0.4)] border border-transparent',
              )}
            >
              {r}
            </button>
          ))}
          {fetchedPoints.length > 0 && (
            <span className="text-[9px] text-[color:var(--color-text-dim)] ml-1 uppercase tracking-wider">historical</span>
          )}
        </div>

        {/* Equity chart */}
        {chartData.length >= 1 ? (
          <div className="h-52 -mx-1 mt-3">
            <ResponsiveContainer width="100%" height="100%">
              <AreaChart data={chartData} margin={{ top: 4, right: 8, bottom: 18, left: 44 }}>
                <defs>
                  <linearGradient id="navGrad" x1="0" y1="0" x2="0" y2="1">
                    <stop offset="0%" stopColor={strokeColor} stopOpacity={0.45} />
                    <stop offset="60%" stopColor={strokeColor} stopOpacity={0.08} />
                    <stop offset="100%" stopColor={strokeColor} stopOpacity={0} />
                  </linearGradient>
                </defs>
                <YAxis
                  domain={['dataMin - 0.05', 'dataMax + 0.05']}
                  tickFormatter={(v: number) => (v >= 0 ? `+${v.toFixed(2)}` : v.toFixed(2))}
                  tick={{ fill: 'oklch(0.55 0.015 260)', fontSize: 9 }}
                  axisLine={false}
                  tickLine={false}
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
                  tick={{ fill: 'oklch(0.55 0.015 260)', fontSize: 9 }}
                  axisLine={false}
                  tickLine={false}
                  height={18}
                />
                <ReferenceLine y={0} stroke="oklch(0.35 0.018 260 / 0.5)" strokeDasharray="3 4" />
                <Tooltip
                  contentStyle={{
                    background: 'oklch(0.17 0.015 260 / 0.95)',
                    backdropFilter: 'blur(12px)',
                    border: '1px solid oklch(0.45 0.02 260 / 0.6)',
                    borderRadius: 8,
                    fontSize: 11,
                    boxShadow: '0 18px 40px -12px oklch(0 0 0 / 0.6)',
                  }}
                  cursor={{ stroke: 'oklch(0.75 0.18 170 / 0.35)', strokeWidth: 1 }}
                  labelFormatter={(label) => fmtChartTime(Number(label))}
                  formatter={(value) => {
                    const v = Number(value ?? 0)
                    return [`${v >= 0 ? '+' : ''}$${Math.abs(v).toFixed(2)}`, 'P&L']
                  }}
                />
                <Area
                  type="monotone"
                  dataKey="v"
                  stroke={strokeColor}
                  strokeWidth={2.25}
                  fill="url(#navGrad)"
                  dot={false}
                  isAnimationActive={false}
                />
              </AreaChart>
            </ResponsiveContainer>
          </div>
        ) : (
          <div className="h-52 flex items-center justify-center text-[color:var(--color-text-dim)] text-sm mt-3 border border-dashed border-[color:var(--color-border-subtle)] rounded-lg">
            Waiting for equity data…
          </div>
        )}

        {/* Stats strip */}
        {portfolio && (
          <div className="grid grid-cols-6 gap-1 mt-4 pt-4 border-t border-[color:var(--color-border-subtle)]">
            <Stat label="Cash" value={`$${fmt(portfolio.cash_usdc, 0)}`} />
            <Stat label="Invested" value={`$${fmt(portfolio.invested_usdc, 0)}`} />
            <Stat label="Fees" value={feesPaid > 0 ? `−$${fmt(feesPaid)}` : '$0.00'} muted={feesPaid === 0} />
            <Stat label="Fill %" value={`${fmt(portfolio.fill_rate_pct, 1)}%`} />
            <Stat label="Win %" value={`${fmt(portfolio.win_rate_pct, 1)}%`} />
            <Stat label="Trades" value={String(portfolio.closed_trades_count ?? 0)} />
          </div>
        )}
      </div>
    </motion.div>
  )
}
