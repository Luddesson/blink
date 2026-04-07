import { usePoll } from '../hooks/usePoll'
import { api } from '../lib/api'
import ErrorBoundary from '../components/ErrorBoundary'
import QualityBadge from '../components/QualityBadge'
import LatencyHistogram from '../components/LatencyHistogram'
import ExecutionKpi from '../components/ExecutionKpi'
import RejectionTrend from '../components/RejectionTrend'
import TwinComparison from '../components/TwinComparison'
import ExperimentPanel from '../components/ExperimentPanel'
import type { PortfolioSummary } from '../types'

interface Props {
  portfolio: PortfolioSummary | undefined
}

export default function PerformancePage({ portfolio }: Props) {
  const { data: latency } = usePoll(api.latency, 5_000)
  const { data: metrics } = usePoll(api.metrics, 5_000)
  const { data: twin } = usePoll(api.twin, 10_000)

  return (
    <div className="flex-1 flex flex-col gap-2 p-2 overflow-y-auto min-h-0">
      {/* Quality badge row */}
      <ErrorBoundary label="QualityBadge">
        <QualityBadge latency={latency} />
      </ErrorBoundary>

      {/* Latency + KPI row */}
      <div className="grid grid-cols-2 gap-2">
        <ErrorBoundary label="LatencyHistogram">
          <LatencyHistogram histogram={latency?.signal_age?.histogram ?? []} />
        </ErrorBoundary>
        <ErrorBoundary label="ExecutionKpi">
          <ExecutionKpi
            portfolio={portfolio ? {
              fill_rate_pct: portfolio.fill_rate_pct,
              reject_rate_pct: portfolio.reject_rate_pct,
              avg_slippage_bps: portfolio.avg_slippage_bps,
            } : null}
            metrics={metrics}
          />
        </ErrorBoundary>
      </div>

      {/* Rejections + Twin row */}
      <div className="grid grid-cols-2 gap-2">
        <ErrorBoundary label="RejectionTrend">
          <RejectionTrend rejectionByReason={metrics?.rejection_by_reason ?? null} />
        </ErrorBoundary>
        <ErrorBoundary label="TwinComparison">
          <TwinComparison
            mainNav={portfolio?.nav_usdc ?? 0}
            mainReturn={portfolio ? ((portfolio.nav_usdc - 250) / 250) * 100 : 0}
            mainWinRate={portfolio?.win_rate_pct ?? 0}
            mainDrawdown={0}
            twin={twin}
          />
        </ErrorBoundary>
      </div>

      {/* Experiments */}
      <ErrorBoundary label="ExperimentPanel">
        <ExperimentPanel />
      </ErrorBoundary>
    </div>
  )
}
