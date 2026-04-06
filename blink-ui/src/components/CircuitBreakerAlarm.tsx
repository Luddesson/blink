import { useState } from 'react'
import { AlertTriangle, RefreshCw, ShieldOff } from 'lucide-react'
import type { RiskSummary } from '../types'
import { api } from '../lib/api'

interface Props {
  risk: RiskSummary
  onDismiss?: () => void
  onReset?: () => void
}

export default function CircuitBreakerAlarm({ risk, onDismiss, onReset }: Props) {
  const [resetting, setResetting] = useState(false)
  const [dismissed, setDismissed] = useState(false)

  if (dismissed) return null
  if (!risk.circuit_breaker_tripped && !risk.circuit_breaker) return null

  const reason = risk.circuit_breaker_reason
  const reasonText = reason
    ? reason.replace(/^VaR breached: /, '').replace(/^Circuit breaker: /, '')
    : 'Trading halted'

  async function handleReset() {
    setResetting(true)
    try {
      await api.resetCircuitBreaker()
      setDismissed(true)
      onReset?.()
    } catch {
      // ignore — will retry
    } finally {
      setResetting(false)
    }
  }

  return (
    <div className="fixed inset-0 z-50 flex flex-col items-center justify-center bg-red-950/95 alarm-shake">
      <ShieldOff size={64} className="text-red-400 mb-4" />
      <h1 className="text-3xl font-bold text-red-300 tracking-widest uppercase mb-2">
        ⚠ Circuit Breaker Tripped
      </h1>
      <p className="text-red-400 text-sm mb-1 max-w-md text-center">{reasonText}</p>
      <p className="text-red-500 text-xs font-mono mb-8">
        Daily P&L: {risk.daily_pnl.toFixed(2)} USDC
      </p>

      <div className="flex gap-3">
        {onDismiss && (
          <button
            onClick={() => { setDismissed(true); onDismiss() }}
            className="flex items-center gap-2 px-5 py-2.5 bg-red-800 hover:bg-red-700 text-red-100 rounded-lg text-sm font-semibold"
          >
            <AlertTriangle size={15} /> Acknowledge (keep halted)
          </button>
        )}
        <button
          onClick={handleReset}
          disabled={resetting}
          className="flex items-center gap-2 px-5 py-2.5 bg-green-700 hover:bg-green-600 disabled:opacity-50 text-white rounded-lg text-sm font-semibold"
        >
          <RefreshCw size={15} className={resetting ? 'animate-spin' : ''} />
          {resetting ? 'Resetting…' : 'Reset & Resume Trading'}
        </button>
      </div>
    </div>
  )
}