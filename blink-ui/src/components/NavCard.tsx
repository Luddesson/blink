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
import { fmt, fmtPct, fmtPnl, pnlClass } from '../lib/format'
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

  const strokeColor = positive
    ? (isLive ? '#f59e0b' : '#818cf8')
    : '#ef4444'

  const chartData = equityCurve.map((v, i) => ({
    t: equityTimestamps[i] ?? i,
    v,
  }))

  const startNav = equityCurve[0] ?? 100

  const uptime = portfolio?.uptime_secs ?? 0
  const uptimeStr = uptime < 3600
    ? `${Math.floor(uptime / 60)}m ${uptime % 60}s`
    : `${Math.floor(uptime / 3600)}h ${Math.floor((uptime % 3600) / 60)}m`

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

      {/* Equity chart */}
      {chartData.length > 1 ? (
        <div className="h-52 -mx-1 mt-3">
          <ResponsiveContainer width="100%" height="100%">
            <AreaChart data={chartData} margin={{ top: 4, right: 4, bottom: 0, left: 0 }}>
              <defs>
                <linearGradient id="navGrad" x1="0" y1="0" x2="0" y2="1">
                  <stop offset="0%" stopColor={strokeColor} stopOpacity={0.28} />
                  <stop offset="100%" stopColor={strokeColor} stopOpacity={0.01} />
                </linearGradient>
              </defs>
              <YAxis domain={['dataMin - 0.3', 'dataMax + 0.3']} hide />
              <XAxis dataKey="t" hide />
              <ReferenceLine y={startNav} stroke="#1e293b" strokeDasharray="4 3" />
              <Tooltip
                contentStyle={{
                  background: '#0f172a',
                  border: '1px solid #334155',
                  borderRadius: 6,
                  fontSize: 11,
                }}
                labelFormatter={(label: number) =>
                  new Date(label).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' })
                }
                formatter={(v: number) => [`$${fmt(v)}`, 'NAV']}
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
        <div className="h-52 flex items-center justify-center text-slate-700 text-sm mt-3 border border-dashed border-surface-700 rounded-lg">
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