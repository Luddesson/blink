import type { ClosedTrade } from '../types'
import { fmt } from '../lib/format'

interface Props {
  trades: ClosedTrade[]
  className?: string
}

export default function PnlAttribution({ trades, className }: Props) {
  if (trades.length === 0) {
    return (
      <div className={`card ${className ?? ''}`}>
        <span className="text-xs font-semibold uppercase tracking-widest text-slate-500 mb-3 block">
          P&L by Market
        </span>
        <p className="text-xs text-slate-500">No closed trades yet</p>
      </div>
    )
  }

  const byMarket = new Map<string, number>()
  for (const t of trades) {
    const key = t.market_title || t.token_id
    byMarket.set(key, (byMarket.get(key) ?? 0) + t.realized_pnl)
  }

  const sorted = [...byMarket.entries()]
    .sort((a, b) => Math.abs(b[1]) - Math.abs(a[1]))
    .slice(0, 10)

  const maxAbs = Math.max(...sorted.map(([, v]) => Math.abs(v)), 1)

  return (
    <div className={`card ${className ?? ''}`}>
      <span className="text-xs font-semibold uppercase tracking-widest text-slate-500 mb-3 block">
        P&L by Market
      </span>
      <div className="space-y-2">
        {sorted.map(([market, pnl]) => {
          const pct = (Math.abs(pnl) / maxAbs) * 100
          const positive = pnl >= 0
          return (
            <div key={market} className="relative">
              <div
                className={`absolute inset-y-0 left-0 rounded ${positive ? 'bg-emerald-500/30' : 'bg-red-500/30'}`}
                style={{ width: `${pct}%` }}
              />
              <div className="relative flex items-center justify-between px-2 py-1">
                <span className="text-xs text-slate-300 truncate max-w-[60%]">{market}</span>
                <span className={`text-xs font-mono ${positive ? 'text-emerald-400' : 'text-red-400'}`}>
                  {pnl >= 0 ? '+' : ''}${fmt(pnl)}
                </span>
              </div>
            </div>
          )
        })}
      </div>
    </div>
  )
}
