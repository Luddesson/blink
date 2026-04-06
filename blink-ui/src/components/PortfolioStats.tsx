import type { PortfolioSummary } from '../types'
import { fmt } from '../lib/format'

interface Props {
  portfolio: PortfolioSummary | undefined
}

function StatRow({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex justify-between text-xs">
      <span className="text-slate-500">{label}</span>
      <span className="font-mono text-slate-300">{value}</span>
    </div>
  )
}

export default function PortfolioStats({ portfolio }: Props) {
  if (!portfolio) {
    return (
      <div className="card space-y-2">
        <span className="text-xs font-semibold uppercase tracking-widest text-slate-500">Stats</span>
        <div className="space-y-1.5">
          {[...Array(5)].map((_, i) => (
            <div key={i} className="h-4 bg-surface-700 rounded animate-pulse" />
          ))}
        </div>
      </div>
    )
  }

  const uptime = portfolio.uptime_secs
  const uptimeStr = uptime < 3600
    ? `${Math.floor(uptime / 60)}m`
    : `${Math.floor(uptime / 3600)}h ${Math.floor((uptime % 3600) / 60)}m`

  return (
    <div className="card">
      <span className="text-xs font-semibold uppercase tracking-widest text-slate-500 mb-3 block">
        Stats
      </span>
      <div className="space-y-2">
        <StatRow label="Fill rate" value={`${fmt(portfolio.fill_rate_pct, 1)}%`} />
        <StatRow label="Win rate" value={`${fmt(portfolio.win_rate_pct, 1)}%`} />
        <StatRow label="Fees paid" value={portfolio.fees_paid_usdc > 0 ? `-$${fmt(portfolio.fees_paid_usdc)}` : '$0.00'} />
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
