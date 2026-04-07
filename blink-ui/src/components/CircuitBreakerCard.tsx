import { ShieldCheck, ShieldOff } from 'lucide-react'
import type { RiskSummary } from '../types'

interface Props {
  risk: RiskSummary
  onReset?: () => void
  className?: string
}

export default function CircuitBreakerCard({ risk, onReset, className }: Props) {
  const tripped = risk.circuit_breaker_tripped ?? risk.circuit_breaker ?? false

  return (
    <div
      className={`card border ${
        tripped
          ? 'border-red-500/60 bg-red-950/20'
          : 'border-emerald-500/40'
      } ${className ?? ''}`}
    >
      <div className="flex items-center gap-2">
        {tripped ? (
          <ShieldOff size={18} className="text-red-400 shrink-0" />
        ) : (
          <ShieldCheck size={18} className="text-emerald-400 shrink-0" />
        )}
        <span
          className={`text-xs font-semibold uppercase tracking-widest ${
            tripped ? 'text-red-400' : 'text-emerald-400'
          }`}
        >
          {tripped ? 'Circuit Breaker Tripped' : 'Circuit Breaker Armed'}
        </span>
      </div>

      {tripped && (
        <div className="mt-3 space-y-3">
          {risk.circuit_breaker_reason && (
            <p className="text-xs text-red-300/80 font-mono">
              {risk.circuit_breaker_reason}
            </p>
          )}
          {onReset && (
            <button
              onClick={onReset}
              className="w-full text-xs font-semibold py-1.5 px-3 rounded bg-red-600 hover:bg-red-500 text-white transition-colors cursor-pointer"
            >
              Reset Circuit Breaker
            </button>
          )}
        </div>
      )}
    </div>
  )
}
