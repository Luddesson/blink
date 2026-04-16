import { useState, useEffect } from 'react'
import type { PortfolioSummary } from '../types'
import { fmt } from '../lib/format'

interface Props {
  portfolio: PortfolioSummary | undefined
}

function StatRow({ label, value, color }: { label: string; value: string; color?: string }) {
  return (
    <div className="flex justify-between text-xs">
      <span className="text-slate-500">{label}</span>
      <span className={`font-mono ${color ?? 'text-slate-300'}`}>{value}</span>
    </div>
  )
}

function pnlColor(v: number) {
  if (v > 0.005) return 'text-emerald-400'
  if (v < -0.005) return 'text-red-400'
  return 'text-slate-400'
}

export default function PortfolioStats({ portfolio }: Props) {
  // Client-side uptime ticker — syncs from WS, increments every second between updates
  const [localUptime, setLocalUptime] = useState(portfolio?.uptime_secs ?? 0)
  useEffect(() => {
    if (portfolio?.uptime_secs != null) setLocalUptime(portfolio.uptime_secs)
  }, [portfolio?.uptime_secs])
  useEffect(() => {
    const id = setInterval(() => setLocalUptime(u => u + 1), 1000)
    return () => clearInterval(id)
  }, [])

  if (!portfolio) {
    return (
      <div className="card space-y-2">
        <span className="text-xs font-semibold uppercase tracking-widest text-slate-500">Portfolio</span>
        <div className="space-y-1.5">
          {[...Array(8)].map((_, i) => (
            <div key={i} className="h-4 bg-surface-700 rounded animate-pulse" />
          ))}
        </div>
      </div>
    )
  }

  const uptimeStr = localUptime < 3600
    ? `${Math.floor(localUptime / 60)}m ${localUptime % 60}s`
    : `${Math.floor(localUptime / 3600)}h ${Math.floor((localUptime % 3600) / 60)}m`

  const cash = portfolio.cash_usdc ?? 0
  const invested = portfolio.invested_usdc ?? 0
  const unrealized = portfolio.unrealized_pnl_usdc ?? 0
  const realized = portfolio.realized_pnl_usdc ?? 0
  const fees = portfolio.fees_paid_usdc ?? 0
  const nav = portfolio.nav_usdc ?? 0
  const netPnl = realized + unrealized - fees

  return (
    <div className="card">
      <span className="text-xs font-semibold uppercase tracking-widest text-slate-500 mb-3 block">
        Portfolio
      </span>
      <div className="space-y-1.5">
        {/* Breakdown */}
        <StatRow label="Cash" value={`$${fmt(cash)}`} />
        <StatRow label="Invested" value={`$${fmt(invested)}`} />
        <StatRow label="Unrealized P&L" value={`${unrealized >= 0 ? '+' : ''}$${fmt(unrealized)}`} color={pnlColor(unrealized)} />
        <StatRow label="Realized P&L" value={`${realized >= 0 ? '+' : ''}$${fmt(realized)}`} color={pnlColor(realized)} />
        <StatRow label="Fees paid" value={fees > 0 ? `-$${fmt(fees)}` : '$0.00'} color={fees > 0 ? 'text-amber-400' : undefined} />
        <div className="border-t border-slate-700/50 my-1.5" />
        <StatRow label="Net P&L" value={`${netPnl >= 0 ? '+' : ''}$${fmt(netPnl)}`} color={pnlColor(netPnl)} />
        <StatRow label="NAV" value={`$${fmt(nav)}`} />
        <div className="border-t border-slate-700/50 my-1.5" />
        {/* Performance */}
        <StatRow label="Fill rate" value={`${fmt(portfolio.fill_rate_pct, 1)}%`} />
        <StatRow label="Win rate" value={`${fmt(portfolio.win_rate_pct, 1)}%`} />
        <StatRow label="Total signals" value={String(portfolio.total_signals ?? 0)} />
        <StatRow label="Filled / Skipped" value={`${portfolio.filled_orders ?? 0} / ${portfolio.skipped_orders ?? 0}`} />
        {portfolio.avg_slippage_bps != null && (
          <StatRow label="Avg slippage" value={`${fmt(portfolio.avg_slippage_bps, 1)} bps`} />
        )}
        <StatRow label="Uptime" value={uptimeStr} />
      </div>
    </div>
  )
}
