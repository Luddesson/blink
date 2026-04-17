import { useState } from 'react'
import { usePoll } from '../hooks/usePoll'
import { useTradeStats, type TradeFilters } from '../hooks/useTradeStats'
import { api } from '../lib/api'
import ErrorBoundary from '../components/ErrorBoundary'
import {
  FilterBar,
  CalendarHeatmap,
  SummaryCards,
  EquityCurveChart,
  RiskMetrics,
  StreakTracker,
  PnlDistributionChart,
  DurationDistributionChart,
  TimeAnalysis,
  SignalSourceComparison,
  ExitReasonBreakdown,
  BiggestTrades,
  MarketBreakdown,
  EnhancedTradeTable,
} from '../components/history'

export default function HistoryPage() {
  const { data: allTrades } = usePoll(() => api.historyAll(), 30_000)
  const [filters, setFilters] = useState<TradeFilters>({})
  const trades = allTrades ?? []
  const stats = useTradeStats(trades, filters)

  return (
    <div className="flex-1 flex flex-col gap-2 p-2 overflow-y-auto min-h-0">
      <ErrorBoundary label="FilterBar">
        <FilterBar filters={filters} onChange={setFilters} totalTrades={stats.totalTrades} />
      </ErrorBoundary>

      <ErrorBoundary label="CalendarHeatmap">
        <CalendarHeatmap dailyPnl={stats.dailyPnl} dailyTrades={stats.dailyTrades} />
      </ErrorBoundary>

      <ErrorBoundary label="SummaryCards">
        <SummaryCards
          totalTrades={stats.totalTrades}
          winRate={stats.winRate}
          totalPnl={stats.totalPnl}
          netPnl={stats.netPnl}
          avgPnl={stats.avgPnl}
          medianPnl={stats.medianPnl}
          avgRiskReward={stats.avgRiskReward}
          profitFactor={stats.profitFactor}
          expectancy={stats.expectancy}
          avgDuration={stats.avgDuration}
          medianDuration={stats.medianDuration}
          totalFees={stats.totalFees}
          avgSlippage={stats.avgSlippage}
        />
      </ErrorBoundary>

      <div className="grid grid-cols-1 xl:grid-cols-3 gap-2">
        <div className="xl:col-span-2">
          <ErrorBoundary label="EquityCurveChart">
            <EquityCurveChart equityCurve={stats.equityCurve} />
          </ErrorBoundary>
        </div>
        <div className="flex flex-col gap-2">
          <ErrorBoundary label="RiskMetrics">
            <RiskMetrics
              maxDrawdown={stats.maxDrawdown}
              maxDrawdownPct={stats.maxDrawdownPct}
              sharpeRatio={stats.sharpeRatio}
              sortinoRatio={stats.sortinoRatio}
              calmarRatio={stats.calmarRatio}
              profitFactor={stats.profitFactor}
            />
          </ErrorBoundary>
          <ErrorBoundary label="StreakTracker">
            <StreakTracker
              currentStreak={stats.currentStreak}
              maxWinStreak={stats.maxWinStreak}
              maxLossStreak={stats.maxLossStreak}
            />
          </ErrorBoundary>
        </div>
      </div>

      <div className="grid grid-cols-1 xl:grid-cols-2 gap-2">
        <ErrorBoundary label="PnlDistributionChart">
          <PnlDistributionChart distribution={stats.pnlDistribution} />
        </ErrorBoundary>
        <ErrorBoundary label="DurationDistributionChart">
          <DurationDistributionChart distribution={stats.durationDistribution} />
        </ErrorBoundary>
      </div>

      <ErrorBoundary label="TimeAnalysis">
        <TimeAnalysis
          pnlByHour={stats.pnlByHour}
          tradesByHour={stats.tradesByHour}
          winRateByHour={stats.winRateByHour}
          pnlByDayOfWeek={stats.pnlByDayOfWeek}
          tradesByDayOfWeek={stats.tradesByDayOfWeek}
          winRateByDayOfWeek={stats.winRateByDayOfWeek}
        />
      </ErrorBoundary>

      <div className="grid grid-cols-1 xl:grid-cols-2 gap-2">
        <ErrorBoundary label="SignalSourceComparison">
          <SignalSourceComparison bySignalSource={stats.bySignalSource} />
        </ErrorBoundary>
        <ErrorBoundary label="ExitReasonBreakdown">
          <ExitReasonBreakdown byExitReason={stats.byExitReason} />
        </ErrorBoundary>
      </div>

      <ErrorBoundary label="BiggestTrades">
        <BiggestTrades top5Wins={stats.top5Wins} top5Losses={stats.top5Losses} />
      </ErrorBoundary>

      <ErrorBoundary label="MarketBreakdown">
        <MarketBreakdown byMarket={stats.byMarket} />
      </ErrorBoundary>

      <ErrorBoundary label="EnhancedTradeTable">
        <EnhancedTradeTable trades={stats.filtered} />
      </ErrorBoundary>
    </div>
  )
}
