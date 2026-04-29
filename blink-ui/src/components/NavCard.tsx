import { useState, useEffect, useRef } from 'react'
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

type TimeRange = '30m' | '1h' | '6h' | '24h' | '7d' | '30d'
const RANGES: TimeRange[] = ['30m', '1h', '6h', '24h', '7d', '30d']

const WINDOW_MS: Record<TimeRange, number> = {
  '30m': 30 * 60 * 1000,
  '1h':  60 * 60 * 1000,
  '6h':  6 * 60 * 60 * 1000,
  '24h': 24 * 60 * 60 * 1000,
  '7d':  7 * 24 * 60 * 60 * 1000,
  '30d': 30 * 24 * 60 * 60 * 1000,
}

interface FetchedPoint { timestamp_ms: number; nav_usdc: number }
interface LiveTruthPoint { t: number; v: number }
interface QuantMetrics {
  win_rate_pct: number
  profit_factor: number
  sharpe_ratio: number
  current_drawdown_pct: number
  total_trades: number
}

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
  const isVerifiedLive = isLive && portfolio?.exchange_positions_verified === true
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
  const [liveTruthPoints, setLiveTruthPoints] = useState<LiveTruthPoint[]>([])
  const lastLiveTruthKey = useRef('')
  const [quant, setQuant] = useState<QuantMetrics | null>(null)

  useEffect(() => {
    if (isLive) {
      setFetchedPoints([])
      setQuant(null)
      return
    }

    let cancelled = false
    const load = async () => {
      try {
        const r = await fetch(`/api/analytics/equity?range=${range}`)
        if (!r.ok) return
        const data = await r.json() as { points: FetchedPoint[] }
        if (!cancelled && Array.isArray(data.points)) setFetchedPoints(data.points)
      } catch { /* engine may not be running */ }
    }
    const loadQuant = async () => {
      try {
        const r = await fetch(`/api/analytics/quant`)
        if (!r.ok) return
        const data = await r.json() as QuantMetrics
        if (!cancelled) setQuant(data)
      } catch {
        if (!cancelled) setQuant(null)
      }
    }
    void load()
    void loadQuant()
    const id = setInterval(() => { void load(); void loadQuant(); }, 30_000)
    return () => { cancelled = true; clearInterval(id) }
  }, [range, isLive])

  useEffect(() => {
    if (!isVerifiedLive) {
      setLiveTruthPoints([])
      lastLiveTruthKey.current = ''
      return
    }

    const checkedAt = typeof portfolio?.truth_checked_at_ms === 'number'
      ? portfolio.truth_checked_at_ms
      : Date.now()
    const livePnl = Number.isFinite(netPnl) ? netPnl : 0
    const key = `${checkedAt}:${livePnl.toFixed(8)}`
    if (lastLiveTruthKey.current === key) return
    lastLiveTruthKey.current = key

    const cutoff = Date.now() - WINDOW_MS['30d']
    setLiveTruthPoints((prev) => {
      const withoutDuplicate = prev.filter((point) => point.t !== checkedAt)
      return [...withoutDuplicate, { t: checkedAt, v: livePnl }]
        .filter((point) => point.t >= cutoff)
        .slice(-2000)
    })
  }, [isVerifiedLive, netPnl, portfolio?.truth_checked_at_ms])

  // Stroke color: bull-emerald when profitable, bear-ruby when down.
  // In live mode, tint toward amber for positive moves.
  const strokeColor = positive
    ? (isLive ? 'oklch(0.78 0.18 65)' : 'oklch(0.78 0.18 155)')
    : 'oklch(0.72 0.22 25)'

  const START_BALANCE = 100
  const rawChartData: { t: number; v: number }[] = isLive
    ? (isVerifiedLive ? liveTruthPoints : [])
    : fetchedPoints.length > 0
      ? fetchedPoints.map(p => ({ t: p.timestamp_ms, v: p.nav_usdc - START_BALANCE }))
      : equityCurve.map((v, i) => ({ t: equityTimestamps[i] ?? i, v: v - START_BALANCE }))

  const chartWindowMs = WINDOW_MS[range]
  const windowStart = nowMs - chartWindowMs

  const filteredData = rawChartData.filter(d => d.t >= windowStart && d.t <= nowMs)
  const currentPnl = isVerifiedLive ? netPnl : nav - START_BALANCE
  const chartTruthBlocked = isLive && !isVerifiedLive
  if (!chartTruthBlocked) {
    if (filteredData.length > 0) {
      const last = filteredData[filteredData.length - 1]
      if (nowMs - last.t > 2000) filteredData.push({ t: nowMs, v: currentPnl })
    } else {
      filteredData.push({ t: windowStart, v: currentPnl })
      filteredData.push({ t: nowMs, v: currentPnl })
    }
  }
  const chartData = chartTruthBlocked ? [] : filteredData

  // Professional: Adjust X-domain to stretch data to fill the graph if we have limited history
  // but stay within the requested window.
  const dataMinT = chartData.length > 0 ? Math.min(...chartData.map(d => d.t)) : windowStart
  const dataMaxT = chartData.length > 0 ? Math.max(...chartData.map(d => d.t)) : nowMs

  // If the user selects a large range but data only exists for a small part, 
  // we zoom in on the data to "fill the graph" as requested.
  const xDomain = [dataMinT, dataMaxT]

  const TICK_COUNT = 5
  const tickInterval = (dataMaxT - dataMinT) / TICK_COUNT
  const xTicks: number[] = Array.from({ length: TICK_COUNT + 1 }, (_, i) =>
    dataMinT + i * tickInterval
  )

  const fmtTickTime = (ms: number) => {
    const date = new Date(ms)
    // If range is large, show date. If we are zoomed in (short duration), show time.
    const duration = dataMaxT - dataMinT
    if (duration > 24 * 3600 * 1000) {
      return `${date.getDate()}/${date.getMonth() + 1}`
    }
    return date.toLocaleTimeString('sv-SE', {
      hour: '2-digit',
      minute: '2-digit',
    })
  }

  const uptimeStr = localUptime < 3600
    ? `${Math.floor(localUptime / 60)}m ${localUptime % 60}s`
    : `${Math.floor(localUptime / 3600)}h ${Math.floor((localUptime % 3600) / 60)}m`
  const exchangePositionValue = portfolio?.exchange_position_value_usdc ?? portfolio?.external_position_value_usdc ?? 0
  const exchangePositionCount = portfolio?.exchange_positions_count ?? 0

  return (
    <motion.div
      initial={{ opacity: 0, y: 10 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.32, ease: [0.2, 0, 0, 1] }}
      className={cn(
        'relative rounded-xl p-5 overflow-visible glass',
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
          <span className="font-sans text-[10px] text-[color:var(--color-text-muted)] uppercase tracking-[0.12em]">
            {isLive ? 'Live wallet PnL' : 'PnL since start'}
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
            {isLive ? 'Wallet NAV' : 'NAV'} <span className="text-[color:var(--color-text-primary)] font-semibold">${fmt(nav)}</span>
          </span>
          {isLive && exchangePositionCount > 0 && (
            <span className="text-xs text-[color:var(--color-text-muted)] font-mono tabular">
              wallet positions <span className="text-[color:var(--color-text-primary)] font-semibold">${fmt(exchangePositionValue)}</span>
            </span>
          )}
          {isLive && (portfolio?.open_positions?.length ?? 0) === 0 && exchangePositionCount > 0 && (
            <span className="text-xs text-[color:var(--color-whale-400)] font-mono tabular">
              Blink positions 0
            </span>
          )}
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
          {isVerifiedLive && liveTruthPoints.length > 0 && (
            <span className="text-[9px] text-[color:var(--color-text-dim)] ml-1 uppercase tracking-wider">wallet truth</span>
          )}
          {!isLive && fetchedPoints.length > 0 && (
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
                  domain={['dataMin - 0.1', 'dataMax + 0.1']}
                  tickFormatter={(v: number) => (v >= 0 ? `+$${v.toFixed(2)}` : `-$${Math.abs(v).toFixed(2)}`)}
                  tick={{ fill: 'oklch(0.55 0.015 260)', fontSize: 9 }}
                  axisLine={false}
                  tickLine={false}
                  width={50}
                />
                <XAxis
                  dataKey="t"
                  type="number"
                  scale="time"
                  domain={xDomain}
                  allowDataOverflow={false}
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
                    backdropFilter: 'blur(16px)',
                    border: '1px solid oklch(0.45 0.02 260 / 0.6)',
                    borderRadius: 12,
                    fontSize: 12,
                    boxShadow: '0 20px 50px -12px oklch(0 0 0 / 0.8)',
                  }}
                  cursor={{ stroke: 'oklch(0.75 0.18 170 / 0.35)', strokeWidth: 1.5 }}
                  labelFormatter={(label) => fmtChartTime(Number(label))}
                  formatter={(value) => {
                    const v = Number(value ?? 0)
                    return [`${v >= 0 ? '+' : '−'}$${Math.abs(v).toFixed(2)}`, 'PnL']
                  }}
                />
                <Area
                  type="monotone"
                  dataKey="v"
                  stroke={strokeColor}
                  strokeWidth={3}
                  fill="url(#navGrad)"
                  dot={false}
                  isAnimationActive={false}
                  activeDot={{ r: 4, strokeWidth: 0, fill: strokeColor }}
                />
              </AreaChart>
            </ResponsiveContainer>
          </div>
        ) : (
          <div className="h-52 flex items-center justify-center text-[color:var(--color-text-dim)] text-sm mt-3 border border-dashed border-[color:var(--color-border-subtle)] rounded-lg">
            {isLive ? 'Waiting for verified wallet truth...' : 'Waiting for equity data...'}
          </div>
        )}

        {/* Stats strip */}
        {portfolio && (
          <div className="grid grid-cols-5 gap-1 mt-4 pt-4 border-t border-[color:var(--color-border-subtle)]">
            {isLive ? (
              <>
                <Stat label="Truth" value={portfolio.reality_status ?? 'unverified'} />
                <Stat label="Positions" value={isVerifiedLive ? String(portfolio.wallet_positions_count ?? portfolio.open_positions.length) : 'unverified'} />
                <Stat label="Value" value={isVerifiedLive ? `$${fmt(portfolio.wallet_position_value_usdc ?? exchangePositionValue)}` : 'unverified'} />
                <Stat label="Basis" value={isVerifiedLive ? `$${fmt(portfolio.wallet_position_initial_value_usdc ?? portfolio.invested_usdc ?? 0)}` : 'unverified'} />
                <Stat label="Open PnL" value={isVerifiedLive ? `${netPnl >= 0 ? '+' : '−'}$${fmt(Math.abs(netPnl))}` : 'unverified'} />
              </>
            ) : (
              <>
                <Stat label="Win Rate" value={`${quant?.win_rate_pct?.toFixed(1) ?? '0.0'}%`} />
                <Stat label="Sharpe" value={quant?.sharpe_ratio?.toFixed(2) ?? '0.00'} />
                <Stat label="Drawdown" value={`${quant?.current_drawdown_pct?.toFixed(1) ?? '0.00'}%`} />
                <Stat label="Profit Factor" value={quant?.profit_factor?.toFixed(2) ?? '1.00'} />
                <Stat label="Trades" value={String(quant?.total_trades ?? 0)} />
              </>
            )}
          </div>
        )}
      </div>
    </motion.div>
  )
}
