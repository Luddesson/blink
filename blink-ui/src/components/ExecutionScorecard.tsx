import type { ClosedTrade } from '../types'
import { fmt, fmtDuration } from '../lib/format'

interface Props {
  trades: ClosedTrade[]
  className?: string
}

export default function ExecutionScorecard({ trades, className }: Props) {
  const count = trades.length
  const avgSlippage = count > 0
    ? trades.reduce((s, t) => s + t.slippage_bps, 0) / count
    : 0
  const avgDuration = count > 0
    ? Math.round(trades.reduce((s, t) => s + t.duration_secs, 0) / count)
    : 0
  const totalFees = trades.reduce((s, t) => s + t.fees_paid_usdc, 0)

  return (
    <div className={`card ${className ?? ''}`}>
      <span className="text-xs font-semibold uppercase tracking-widest text-slate-500 mb-3 block">
        Execution Scorecard
      </span>
      <div className="grid grid-cols-4 gap-4">
        <Cell label="Avg Slippage" value={`${fmt(avgSlippage, 1)} bps`} />
        <Cell label="Avg Duration" value={fmtDuration(avgDuration)} />
        <Cell label="Total Fees" value={`$${fmt(totalFees)}`} />
        <Cell label="Trades Scored" value={String(count)} />
      </div>
    </div>
  )
}

function Cell({ label, value }: { label: string; value: string }) {
  return (
    <div>
      <div className="text-[10px] uppercase tracking-wide text-slate-500">{label}</div>
      <div className="text-sm font-mono text-slate-100">{value}</div>
    </div>
  )
}
