import type { BullpenConvergenceResponse } from '../types'
import { fmt } from '../lib/format'

interface Props {
  convergence: BullpenConvergenceResponse | null
  variant?: 'compact' | 'detailed'
}

export default function ConvergencePanel({ convergence, variant = 'detailed' }: Props) {
  if (!convergence || !convergence.enabled) {
    if (variant === 'detailed') {
      return (
        <div className="card border border-slate-800 bg-slate-900">
          <p className="text-xs text-slate-600">Bullpen not enabled</p>
        </div>
      )
    }
    return null
  }

  const signals = convergence.signals ?? []

  // Compact variant — minimal, alert-style
  if (variant === 'compact') {
    if (signals.length === 0) return null

    return (
      <div className="card border border-amber-800/40 bg-amber-950/20">
        <div className="flex items-center gap-1.5 mb-2">
          <span className="text-amber-400 text-sm">🐋</span>
          <span className="text-xs font-semibold uppercase tracking-widest text-amber-400">
            Convergence
          </span>
          <span className="text-[10px] text-amber-500 ml-auto">
            {convergence.active_signals ?? signals.length} active
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

  // Detailed variant — full featured
  const hasSignals = signals.length > 0

  return (
    <div
      className={`card border ${
        hasSignals ? 'border-amber-700/50 bg-amber-950/10' : 'border-slate-800 bg-slate-900'
      }`}
    >
      {/* Header */}
      <div className="flex items-center gap-2 mb-3">
        <span className="text-base">🐋</span>
        <span className="text-xs font-semibold uppercase tracking-widest text-slate-300">
          Whale Convergence
        </span>
        {hasSignals && (
          <span className="ml-auto px-1.5 py-0.5 rounded bg-amber-800/40 text-amber-400 text-[10px] font-medium tabular-nums">
            {signals.length} signal{signals.length !== 1 ? 's' : ''}
          </span>
        )}
      </div>

      {/* Signals */}
      {!hasSignals ? (
        <p className="text-[11px] text-slate-600">
          No convergence detected — monitoring whale activity
        </p>
      ) : (
        <div className="space-y-2">
          {signals.map((s, i) => {
            const isBuy = s.net_direction === 'Buy'
            const pct = s.convergence_score * 100

            return (
              <div
                key={i}
                className="rounded bg-slate-900/60 border border-slate-800/50 px-3 py-2"
              >
                <div className="flex items-start gap-2">
                  {/* Direction arrow */}
                  <span
                    className={`text-lg font-bold leading-none mt-0.5 ${
                      isBuy ? 'text-emerald-400' : 'text-red-400'
                    }`}
                  >
                    {isBuy ? '▲' : '▼'}
                  </span>

                  <div className="flex-1 min-w-0">
                    {/* Market title */}
                    <p
                      className="text-[11px] text-slate-200 truncate font-medium"
                      title={s.market_title ?? ''}
                    >
                      {s.market_title ?? 'Unknown Market'}
                    </p>

                    {/* Stats row */}
                    <p className="text-[11px] text-slate-500 mt-0.5">
                      <span className="text-slate-400 tabular-nums">{s.wallet_count}</span> wallets
                      {' · '}
                      <span className="text-slate-400 tabular-nums">${fmt(s.total_usd, 0)}</span>
                      {' · '}
                      <span className="text-amber-400 tabular-nums">{fmt(pct, 0)}%</span> convergence
                    </p>

                    {/* Convergence bar */}
                    <div className="mt-1.5 h-1 rounded-full bg-slate-800 overflow-hidden max-w-[200px]">
                      <div
                        className="h-full rounded-full bg-amber-500"
                        style={{ width: `${pct}%` }}
                      />
                    </div>
                  </div>
                </div>
              </div>
            )
          })}
        </div>
      )}
    </div>
  )
}
