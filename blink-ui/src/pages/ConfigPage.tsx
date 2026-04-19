import ErrorBoundary from '../components/ErrorBoundary'
import RiskConfigForm from '../components/RiskConfigForm'
import CircuitBreakerCard from '../components/CircuitBreakerCard'
import { api } from '../lib/api'
import type { RiskSummary } from '../types'

interface Props {
  risk: RiskSummary
  connected: boolean
}

export default function ConfigPage({ risk, connected }: Props) {
  const handleResetCb = async () => {
    try {
      await api.resetCircuitBreaker()
    } catch (e) {
      console.error('Failed to reset circuit breaker:', e)
    }
  }

  if (!connected) {
    return (
      <div className="flex-1 flex items-center justify-center p-2 min-h-0">
        <div className="rounded-lg border border-[color:oklch(0.60_0.15_270/0.3)] bg-gradient-to-br from-[color:oklch(0.30_0.08_270/0.4)] to-[color:oklch(0.25_0.06_270/0.3)] backdrop-blur-lg p-6 flex flex-col items-center gap-4">
          <div className="flex items-center gap-3">
            <div className="w-2 h-2 rounded-full bg-[color:var(--color-bull-400)] animate-pulse" />
            <span className="text-sm font-medium text-[color:var(--color-text-secondary)]">Connecting to engine…</span>
          </div>
          <div className="w-48 h-32 opacity-40 animate-pulse rounded-md bg-gradient-to-r from-[color:oklch(0.40_0.10_270/0.2)] to-[color:oklch(0.35_0.08_270/0.2)]" />
        </div>
      </div>
    )
  }

  return (
    <div className="flex-1 grid min-h-0 grid-cols-1 gap-2 overflow-y-auto p-2 xl:grid-cols-2">
      <div className="flex flex-col gap-2">
        <ErrorBoundary label="RiskConfigForm">
          <RiskConfigForm risk={risk} />
        </ErrorBoundary>
      </div>

      <div className="flex flex-col gap-2">
        <ErrorBoundary label="CircuitBreakerCard">
          <CircuitBreakerCard risk={risk} onReset={handleResetCb} />
        </ErrorBoundary>
      </div>
    </div>
  )
}
