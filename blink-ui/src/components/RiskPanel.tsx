import { ShieldCheck, ShieldOff } from 'lucide-react'
import type { RiskSummary } from '../types'
import { fmt, fmtPnl, pnlClass } from '../lib/format'

interface Props { risk: RiskSummary }

export default function RiskPanel({ risk }: Props) {
  // Derive daily loss limit from percentage (applied to typical NAV of ~$250)
  const pnl = risk.daily_pnl ?? 0
  const maxLossPct = risk.max_daily_loss_pct ?? 0.05
  const cbTripped = risk.circuit_breaker_tripped ?? risk.circuit_breaker ?? false

  return (
    <div className="card">
      <div className="flex items-center justify-between mb-3">
        <span className="text-xs font-semibold uppercase tracking-widest text-slate-500">
          Risk
        </span>
        {cbTripped ? (
          <span className="badge badge-danger flex items-center gap-1">
            <ShieldOff size={10} /> CB TRIPPED
          </span>
        ) : (
          <span className="badge badge-ok flex items-center gap-1">
            <ShieldCheck size={10} /> CB OK
          </span>
        )}
      </div>

      <div className="space-y-3">
        {/* Daily P&L + progress bar toward daily loss limit */}
        <div>
          <div className="flex justify-between text-xs mb-1">
            <span className="text-slate-500">Daily P&amp;L</span>
            <span className={`font-mono font-semibold ${pnlClass(pnl)}`}>
              {fmtPnl(pnl)} USDC
            </span>
          </div>
          {/* Progress bar: how close to hitting the daily loss limit */}
          {maxLossPct > 0 && (() => {
            // We approximate nav from daily_pnl; the ratio is what matters
            const limitUSDC = 250 * maxLossPct  // approx $12.50 at 5%
            const usedPct = Math.max(0, Math.min(1, Math.abs(pnl < 0 ? pnl / limitUSDC : 0)))
            const barColor = usedPct > 0.75 ? 'bg-red-500' : usedPct > 0.4 ? 'bg-yellow-500' : 'bg-emerald-500'
            return (
              <div className="w-full bg-surface-700 rounded-full h-1 mt-1">
                <div
                  className={`h-1 rounded-full transition-all ${barColor}`}
                  style={{ width: `${usedPct * 100}%` }}
                />
              </div>
            )
          })()}
        </div>

        <div className="flex justify-between text-xs">
          <span className="text-slate-500">Max daily loss</span>
          <span className="font-mono text-slate-300">{fmt(maxLossPct * 100, 1)}%</span>
        </div>
        {risk.max_concurrent_positions != null && (
          <div className="flex justify-between text-xs">
            <span className="text-slate-500">Max concurrent</span>
            <span className="font-mono text-slate-300">{risk.max_concurrent_positions}</span>
          </div>
        )}
        {risk.max_single_order_usdc != null && (
          <div className="flex justify-between text-xs">
            <span className="text-slate-500">Max order size</span>
            <span className="font-mono text-slate-300">${fmt(risk.max_single_order_usdc)}</span>
          </div>
        )}
        <div className="flex justify-between text-xs">
          <span className="text-slate-500">Trading enabled</span>
          <span className={`font-mono font-semibold ${risk.trading_enabled ? 'text-emerald-400' : 'text-red-400'}`}>
            {risk.trading_enabled ? 'YES' : 'NO'}
          </span>
        </div>
      </div>
    </div>
  )
}
