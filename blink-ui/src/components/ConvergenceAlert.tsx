import type { BullpenConvergenceResponse } from '../types'
import { fmt } from '../lib/format'

interface Props { convergence: BullpenConvergenceResponse | null }

export default function ConvergenceAlert({ convergence }: Props) {
  if (!convergence || !convergence.enabled) return null

  const signals = convergence.active_signals ?? []
  if (signals.length === 0) return null

  return (
    <div className="card border border-amber-800/40 bg-amber-950/20">
      <div className="flex items-center gap-1.5 mb-2">
        <span className="text-amber-400 text-sm">🐋</span>
        <span className="text-xs font-semibold uppercase tracking-widest text-amber-400">
          Convergence
        </span>
        <span className="text-[10px] text-amber-500 ml-auto">
          {signals.length} active
        </span>
      </div>

      <div className="space-y-1.5">
        {signals.map((s, i) => (
          <div key={i} className="text-[11px] flex items-start gap-1.5">
            <span
              className={`shrink-0 font-medium ${
                s.net_direction === 'Buy' ? 'text-emerald-400' : 'text-red-400'
              }`}
            >
              {s.net_direction === 'Buy' ? '▲' : '▼'}
            </span>
            <div className="flex-1 min-w-0">
              <p className="text-slate-300 truncate" title={s.market_title ?? ''}>
                {s.market_title ?? 'Unknown'}
              </p>
              <p className="text-slate-500">
                {s.wallet_count} wallets · ${fmt(s.total_usd, 0)} ·{' '}
                <span className="text-amber-400">{fmt(s.convergence_score * 100, 0)}%</span>
              </p>
            </div>
          </div>
        ))}
      </div>
    </div>
  )
}
