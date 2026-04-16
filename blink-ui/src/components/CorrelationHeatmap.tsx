import type { Position } from '../types'
import { fmt } from '../lib/format'

interface Props {
  positions: Position[]
  className?: string
}

/** Pearson-style "synthetic" correlation between two positions.
 *  Uses side alignment (+/–) and P&L direction as a proxy.
 *  Returns a value in [-1, 1]. */
function syntheticCorr(a: Position, b: Position): number {
  const sameMarket = a.market_title && b.market_title && a.market_title === b.market_title
  if (sameMarket) return 1.0

  const sameSide = a.side === b.side ? 1 : -1
  const pnlA = a.unrealized_pnl ?? 0
  const pnlB = b.unrealized_pnl ?? 0
  const bothPos = pnlA > 0 && pnlB > 0
  const bothNeg = pnlA < 0 && pnlB < 0
  const pnlAlign = bothPos || bothNeg ? 0.5 : -0.3

  return Math.max(-1, Math.min(1, sameSide * 0.4 + pnlAlign))
}

function cellColor(corr: number): string {
  if (corr >= 0.7) return 'bg-rose-700/70 text-rose-200'
  if (corr >= 0.4) return 'bg-amber-700/60 text-amber-200'
  if (corr >= 0.1) return 'bg-slate-700/60 text-slate-300'
  if (corr >= -0.2) return 'bg-slate-800 text-slate-500'
  return 'bg-emerald-900/60 text-emerald-300'
}

function shortLabel(pos: Position): string {
  if (pos.market_title) return pos.market_title.slice(0, 10)
  return pos.token_id.slice(0, 8)
}

const MAX = 8

export default function CorrelationHeatmap({ positions, className }: Props) {
  const pts = positions.slice(0, MAX)

  if (pts.length < 2) {
    return (
      <div className={`card ${className ?? ''}`}>
        <span className="text-xs font-semibold uppercase tracking-widest text-slate-500 mb-3 block">
          Position Correlation
        </span>
        <p className="text-xs text-slate-600 italic">Need ≥ 2 open positions</p>
      </div>
    )
  }

  return (
    <div className={`card ${className ?? ''}`}>
      <span className="text-xs font-semibold uppercase tracking-widest text-slate-500 mb-3 block">
        Position Correlation
      </span>
      <div className="overflow-x-auto">
        <table className="text-[10px] border-collapse">
          <thead>
            <tr>
              <th className="w-20" />
              {pts.map((p) => (
                <th
                  key={p.id}
                  className="px-1 py-0.5 text-slate-400 font-normal truncate max-w-[56px] text-center"
                  title={p.market_title ?? p.token_id}
                >
                  {shortLabel(p)}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {pts.map((row) => (
              <tr key={row.id}>
                <td
                  className="pr-2 text-slate-400 truncate max-w-[80px]"
                  title={row.market_title ?? row.token_id}
                >
                  {shortLabel(row)}
                </td>
                {pts.map((col) => {
                  const isDiag = row.id === col.id
                  const corr = isDiag ? 1.0 : syntheticCorr(row, col)
                  return (
                    <td
                      key={col.id}
                      className={`text-center px-1 py-0.5 rounded-sm tabular-nums font-mono ${isDiag ? 'bg-slate-700 text-slate-200' : cellColor(corr)}`}
                      title={`${shortLabel(row)} ↔ ${shortLabel(col)}: ${fmt(corr, 2)}`}
                    >
                      {isDiag ? '—' : fmt(corr, 2)}
                    </td>
                  )
                })}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <div className="mt-2 flex gap-3 text-[9px] text-slate-500">
        <span><span className="inline-block w-2 h-2 rounded-sm bg-rose-700/70 mr-0.5" />high</span>
        <span><span className="inline-block w-2 h-2 rounded-sm bg-amber-700/60 mr-0.5" />medium</span>
        <span><span className="inline-block w-2 h-2 rounded-sm bg-emerald-900/60 mr-0.5" />inverse</span>
      </div>
    </div>
  )
}
