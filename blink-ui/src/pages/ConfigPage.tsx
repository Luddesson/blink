import ErrorBoundary from '../components/ErrorBoundary'
import RiskConfigForm from '../components/RiskConfigForm'
import CircuitBreakerCard from '../components/CircuitBreakerCard'
import { api } from '../lib/api'
import type { RiskSummary } from '../types'

interface Props {
  risk: RiskSummary
}

export default function ConfigPage({ risk }: Props) {
  const handleResetCb = async () => {
    try {
      await api.resetCircuitBreaker()
    } catch (e) {
      console.error('Failed to reset circuit breaker:', e)
    }
  }

  return (
    <div className="flex-1 grid grid-cols-2 gap-2 p-2 overflow-y-auto min-h-0">
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
