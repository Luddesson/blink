import { useCallback, useState } from 'react'
import ErrorBoundary from '../components/ErrorBoundary'
import RiskConfigForm from '../components/RiskConfigForm'
import CircuitBreakerCard from '../components/CircuitBreakerCard'
import { usePoll } from '../hooks/usePoll'
import { api } from '../lib/api'
import type { RiskSummary, StrategyMode } from '../types'

interface Props {
  risk: RiskSummary
  connected: boolean
}

export default function ConfigPage({ risk, connected }: Props) {
  const { data: status } = usePoll(api.status, 5_000, connected)
  const strategy = status?.strategy
  const [submittingStrategy, setSubmittingStrategy] = useState(false)
  const [strategyError, setStrategyError] = useState<string | null>(null)
  const cooldownRemainingMs = Math.max(
    0,
    (strategy?.last_switched_at_ms ?? 0) + (strategy?.cooldown_secs ?? 0) * 1_000 - Date.now(),
  )
  const cooldownActive = cooldownRemainingMs > 0
  const cooldownRemainingSecs = Math.ceil(cooldownRemainingMs / 1_000)

  const handleResetCb = async () => {
    try {
      await api.resetCircuitBreaker()
    } catch (e) {
      console.error('Failed to reset circuit breaker:', e)
    }
  }

  const handleStrategyChange = useCallback(async (mode: StrategyMode) => {
    if (!strategy || strategy.current_mode === mode || cooldownActive) return
    setStrategyError(null)
    setSubmittingStrategy(true)
    try {
      await api.setStrategy(mode, `operator-switch:${mode}`)
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      setStrategyError(message.includes('HTTP 429') ? `Switch cooldown active (${cooldownRemainingSecs}s).` : message)
    } finally {
      setSubmittingStrategy(false)
    }
  }, [cooldownActive, cooldownRemainingSecs, strategy])

  const handleStrategyRollback = useCallback(async () => {
    setStrategyError(null)
    setSubmittingStrategy(true)
    try {
      await api.rollbackStrategy('operator-rollback-to-mirror')
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      setStrategyError(message)
    } finally {
      setSubmittingStrategy(false)
    }
  }, [])

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
        <section className="rounded-lg border border-[color:var(--color-border-primary)] bg-[color:var(--color-surface-elevated)] p-3 shadow-[0_12px_40px_rgba(0,0,0,0.2)]">
          <div className="flex items-center justify-between gap-2">
            <h3 className="text-sm font-semibold text-[color:var(--color-text-primary)]">Strategy Mode</h3>
            <span className="text-xs text-[color:var(--color-text-muted)]">
              Active: <strong className="text-[color:var(--color-text-secondary)]">{strategy?.current_mode ?? 'n/a'}</strong>
            </span>
          </div>
          <div className="mt-2 flex flex-wrap gap-2">
            {(['mirror', 'conservative', 'aggressive'] as const).map((mode) => (
              <button
                key={mode}
                type="button"
                disabled={submittingStrategy || cooldownActive || !strategy?.runtime_switch_enabled || strategy?.current_mode === mode}
                onClick={() => void handleStrategyChange(mode)}
                className="rounded-md border border-[color:var(--color-border-primary)] bg-[color:var(--color-surface-subtle)] px-3 py-1.5 text-xs font-medium text-[color:var(--color-text-secondary)] transition enabled:hover:border-[color:var(--color-bull-500)] enabled:hover:text-[color:var(--color-text-primary)] disabled:cursor-not-allowed disabled:opacity-50"
              >
                {mode}
              </button>
            ))}
              <button
                type="button"
                disabled={submittingStrategy || cooldownActive}
                onClick={() => void handleStrategyRollback()}
                className="rounded-md border border-[color:var(--color-bear-500)] bg-[color:oklch(0.32_0.10_24/0.24)] px-3 py-1.5 text-xs font-medium text-[color:var(--color-bear-300)] transition enabled:hover:bg-[color:oklch(0.35_0.12_24/0.35)] disabled:cursor-not-allowed disabled:opacity-50"
              >
                Rollback to mirror
            </button>
          </div>
          <p className="mt-2 text-[11px] text-[color:var(--color-text-muted)]">
            Runtime switch: {strategy?.runtime_switch_enabled ? 'enabled' : 'disabled'} • Cooldown: {strategy?.cooldown_secs ?? 0}s
          </p>
          {cooldownActive && (
            <p className="mt-1 text-[11px] text-[color:var(--color-bear-300)]">
              Cooldown active: wait {cooldownRemainingSecs}s before next switch.
            </p>
          )}
          {strategyError && (
            <p className="mt-2 rounded-md border border-[color:var(--color-bear-500)] bg-[color:oklch(0.30_0.12_24/0.30)] px-2 py-1.5 text-xs text-[color:var(--color-bear-300)]">
              {strategyError}
            </p>
          )}
        </section>
      </div>
    </div>
  )
}
