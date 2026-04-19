import { useMemo } from 'react'
import { motion } from 'motion/react'
import { usePoll } from '../hooks/usePoll'
import { api } from '../lib/api'
import ErrorBoundary from '../components/ErrorBoundary'
import QualityBadge from '../components/QualityBadge'
import LatencyHistogram from '../components/LatencyHistogram'
import ExecutionKpi from '../components/ExecutionKpi'
import RejectionTrend from '../components/RejectionTrend'
import ExposureHeatmap from '../components/ExposureHeatmap'
import CorrelationHeatmap from '../components/CorrelationHeatmap'
import { fmt, fmtPnl, fmtDuration, pnlClass } from '../lib/format'
import type { PortfolioSummary, Position } from '../types'

interface Props {
  portfolio: PortfolioSummary | undefined
  positions?: Position[]
}

export default function PerformancePage({ portfolio, positions = [] }: Props) {
  const { data: latency } = usePoll(api.latency, 5_000)
  const { data: metrics } = usePoll(api.metrics, 5_000)

  const nav = portfolio?.nav_usdc ?? 0
  const startNav = 100
  const totalPnl = nav - startNav
  const realizedPnl = portfolio?.realized_pnl_usdc ?? 0
  const unrealizedPnl = portfolio?.unrealized_pnl_usdc ?? 0
  const feesPaid = portfolio?.fees_paid_usdc ?? 0
  const winRate = portfolio?.win_rate_pct ?? 0
  const closedCount = portfolio?.closed_trades_count ?? 0
  const totalSignals = portfolio?.total_signals ?? 0
  const openCount = positions.length
  const uptime = portfolio?.uptime_secs ?? 0

  const isWarmingUp = useMemo(() => {
    return !latency?.signal_age || (latency.signal_age.count ?? 0) < 10
  }, [latency?.signal_age])

  const sampleCount = useMemo(() => {
    return latency?.signal_age?.count ?? 0
  }, [latency?.signal_age?.count])

  return (
    <div className="flex-1 flex flex-col gap-2 p-2 overflow-y-auto min-h-0">
      {/* Quality badge row */}
      <ErrorBoundary label="QualityBadge">
        <div className="flex flex-col gap-2">
          <QualityBadge latency={latency} />
          {isWarmingUp && (
            <motion.div
              initial={{ opacity: 0 }}
              animate={{ opacity: 1 }}
              transition={{ duration: 0.3 }}
              className="text-xs text-slate-400 font-mono pl-1"
            >
              Collecting signal samples… ({sampleCount} / 10)
            </motion.div>
          )}
        </div>
      </ErrorBoundary>

      {/* Latency + KPI row */}
      <div className="grid grid-cols-1 gap-2 lg:grid-cols-2">
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

      {/* Rejections + Exposure row */}
      <div className="grid grid-cols-1 gap-2 lg:grid-cols-2">
        <ErrorBoundary label="RejectionTrend">
          <RejectionTrend rejectionByReason={metrics?.rejection_by_reason ?? null} />
        </ErrorBoundary>
        <ErrorBoundary label="ExposureHeatmap">
          <ExposureHeatmap positions={positions} />
        </ErrorBoundary>
      </div>

      {/* Correlation heatmap */}
      {positions.length >= 2 && (
        <ErrorBoundary label="CorrelationHeatmap">
          <CorrelationHeatmap positions={positions} />
        </ErrorBoundary>
      )}

      {/* Trading stats */}
      <div className="card">
        <span className="text-xs font-semibold uppercase tracking-widest text-slate-500 block mb-3">
          Session Statistics
        </span>
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 xl:grid-cols-4">
          <div className="bg-surface-900 rounded-lg px-3 py-2">
            <div className="text-[10px] text-slate-500 uppercase tracking-wider mb-0.5">Total P&amp;L</div>
            <div className={`font-mono font-bold text-lg ${pnlClass(totalPnl)}`}>{fmtPnl(totalPnl)}</div>
            <div className="text-[10px] text-slate-600 mt-0.5">
              R: <span className={pnlClass(realizedPnl)}>{fmtPnl(realizedPnl)}</span>
              {' '}U: <span className={pnlClass(unrealizedPnl)}>{fmtPnl(unrealizedPnl)}</span>
            </div>
          </div>
          <div className="bg-surface-900 rounded-lg px-3 py-2">
            <div className="text-[10px] text-slate-500 uppercase tracking-wider mb-0.5">Win Rate</div>
            <div className={`font-mono font-bold text-lg ${winRate >= 55 ? 'text-emerald-400' : winRate >= 45 ? 'text-amber-400' : 'text-red-400'}`}>
              {fmt(winRate, 1)}%
            </div>
            <div className="text-[10px] text-slate-600 mt-0.5">{closedCount} closed trades</div>
          </div>
          <div className="bg-surface-900 rounded-lg px-3 py-2">
            <div className="text-[10px] text-slate-500 uppercase tracking-wider mb-0.5">Fees Paid</div>
            <div className="font-mono font-bold text-lg text-rose-400">${fmt(feesPaid, 4)}</div>
            <div className="text-[10px] text-slate-600 mt-0.5">
              {totalPnl !== 0 ? `${fmt(Math.abs(feesPaid / (Math.abs(totalPnl) + feesPaid)) * 100, 1)}% drag` : '—'}
            </div>
          </div>
          <div className="bg-surface-900 rounded-lg px-3 py-2">
            <div className="text-[10px] text-slate-500 uppercase tracking-wider mb-0.5">Activity</div>
            <div className="font-mono font-bold text-lg text-slate-200">{openCount} open</div>
            <div className="text-[10px] text-slate-600 mt-0.5">
              {totalSignals} signals · {fmtDuration(uptime)} up
            </div>
          </div>
        </div>
      </div>
    </div>
  )
}
