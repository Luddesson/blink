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
  const [range, setRange] = useState<'24h' | '7d' | '30d' | 'all'>('24h')
  const { data: allTrades } = usePoll(() => api.historyAll(range), 60_000, true, [range])
  const [filters, setFilters] = useState<TradeFilters>({})
  const trades = allTrades ?? []
  const stats = useTradeStats(trades, filters)

  return (
    <div className="flex-1 flex flex-col gap-2 p-2 overflow-y-auto min-h-0">
      <div className="flex justify-between items-center bg-neutral-900/50 p-2 rounded border border-neutral-800">
        <div className="text-sm font-bold text-neutral-400 uppercase tracking-widest ml-2">Trade History</div>
        <div className="flex gap-1 bg-neutral-950 p-1 rounded border border-neutral-800">
          {(['24h', '7d', '30d', 'all'] as const).map((r) => (
            <button
              key={r}
              onClick={() => setRange(r)}
              className={`px-3 py-1 text-[10px] uppercase font-bold rounded transition-colors ${
                range === r
                  ? 'bg-emerald-500/20 text-emerald-400 border border-emerald-500/30'
                  : 'text-neutral-500 hover:text-neutral-300'
              }`}
            >
              {r}
            </button>
          ))}
        </div>
      </div>

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
