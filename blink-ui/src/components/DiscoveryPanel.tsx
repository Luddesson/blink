import type { BullpenDiscoveryResponse } from '../types'
import { fmt } from '../lib/format'

interface Props { discovery: BullpenDiscoveryResponse | null }

export default function DiscoveryPanel({ discovery }: Props) {
  if (!discovery || !discovery.enabled) return null

  const markets = discovery.markets ?? []
  const top5 = [...markets]
    .sort((a, b) => b.viability_score - a.viability_score)
    .slice(0, 5)

  return (
    <div className="card">
      <div className="flex items-center justify-between mb-2">
        <span className="text-xs font-semibold uppercase tracking-widest text-slate-500">
          Discovery
        </span>
        <span className="text-[10px] text-slate-500">
          {discovery.total_markets ?? 0} mkts · scan #{discovery.scan_count ?? 0}
        </span>
      </div>

      {top5.length === 0 ? (
        <p className="text-[11px] text-slate-600">No markets discovered yet</p>
      ) : (
        <div className="space-y-1.5">
          {top5.map((m) => (
            <div key={m.token_id} className="flex items-center gap-1.5 text-[11px]">
              <div
                className="w-1 h-3 rounded-sm shrink-0"
                style={{
                  backgroundColor: `hsl(${Math.round(m.viability_score * 120)}, 70%, 50%)`,
                }}
              />
              <span className="text-slate-300 truncate flex-1" title={m.token_id}>
                {m.lenses.join('·')}
              </span>
              {m.smart_money_interest && (
                <span className="text-amber-400 text-[9px]">🐋</span>
              )}
              <span className="text-slate-400 tabular-nums">
                {fmt(m.viability_score * 100, 0)}%
              </span>
            </div>
          ))}
        </div>
      )}
    </div>
  )
}
