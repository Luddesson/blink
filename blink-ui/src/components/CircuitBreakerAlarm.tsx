import { AlertTriangle, ShieldOff } from 'lucide-react'
import type { RiskSummary } from '../types'

interface Props {
  risk: RiskSummary
  onDismiss?: () => void
}

export default function CircuitBreakerAlarm({ risk, onDismiss }: Props) {
  if (!risk.circuit_breaker_tripped && !risk.circuit_breaker) return null

  return (
    <div className="fixed inset-0 z-50 flex flex-col items-center justify-center bg-red-950/95 alarm-shake">
      <ShieldOff size={64} className="text-red-400 mb-4" />
      <h1 className="text-3xl font-bold text-red-300 tracking-widest uppercase mb-2">
        ⚠ Circuit Breaker Tripped
      </h1>
      <p className="text-red-400 text-sm mb-1">
        Daily loss limit exceeded — all new orders are halted
      </p>
      <p className="text-red-500 text-xs font-mono mb-8">
        Daily P&amp;L: {risk.daily_pnl.toFixed(2)} USDC
      </p>

      <div className="flex gap-3">
        {onDismiss && (
          <button
            onClick={onDismiss}
            className="flex items-center gap-2 px-5 py-2.5 bg-red-800 hover:bg-red-700 text-red-100 rounded-lg text-sm font-semibold"
          >
            <AlertTriangle size={15} /> Acknowledge (keep halted)
          </button>
        )}
        <p className="self-center text-red-600 text-xs">
          Reset requires operator action in engine config
        </p>
      </div>
    </div>
  )
}
