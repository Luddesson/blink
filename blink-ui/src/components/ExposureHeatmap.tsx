import type { Position } from '../types'
import { fmt } from '../lib/format'

interface Props {
  positions: Position[]
  className?: string
}

const MAX_ROWS = 15

function barColor(ratio: number): string {
  if (ratio > 0.8) return '#f92672'
  if (ratio > 0.5) return '#e6db74'
  return '#a6e22e'
}

export default function ExposureHeatmap({ positions, className }: Props) {
  const sorted = [...positions]
    .sort((a, b) => b.usdc_spent - a.usdc_spent)
    .slice(0, MAX_ROWS)

  const maxSpent = sorted.length > 0 ? sorted[0].usdc_spent : 0

  return (
    <div className={`card ${className ?? ''}`}>
      <span className="text-xs font-semibold uppercase tracking-widest text-slate-500 mb-3 block">
        Exposure Heatmap
      </span>

      {sorted.length === 0 ? (
        <p className="text-xs text-slate-600 italic">No open positions</p>
      ) : (
        <div className="space-y-1.5">
          {sorted.map((pos) => {
            const ratio = maxSpent > 0 ? pos.usdc_spent / maxSpent : 0
            const label =
              pos.market_title ??
              `${pos.token_id.slice(0, 8)}…${pos.token_id.slice(-4)}`

            return (
              <div key={pos.id} className="flex items-center gap-2 text-xs">
                <span className="text-slate-400 truncate w-28 shrink-0" title={label}>
                  {label}
                </span>
                <div className="flex-1 h-3 bg-slate-800 rounded-sm overflow-hidden">
                  <div
                    className="h-full rounded-sm transition-all"
                    style={{
                      width: `${ratio * 100}%`,
                      backgroundColor: barColor(ratio),
                    }}
                  />
                </div>
                <span className="text-slate-300 font-mono tabular-nums w-16 text-right shrink-0">
                  ${fmt(pos.usdc_spent)}
                </span>
              </div>
            )
          })}
        </div>
      )}
    </div>
  )
}
