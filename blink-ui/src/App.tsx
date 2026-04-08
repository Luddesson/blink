import { useState, useCallback } from 'react'
import { useEngineSocket } from './hooks/useEngineSocket'
import { usePoll } from './hooks/usePoll'
import { useMode } from './hooks/useMode'
import { useTab } from './hooks/useTab'
import { useKeyboard } from './hooks/useKeyboard'
import { api } from './lib/api'

import Header from './components/Header'
import TabBar from './components/TabBar'
import StatusBar from './components/StatusBar'
import NavCard from './components/NavCard'
import RiskPanel from './components/RiskPanel'
import EmergencyStop from './components/EmergencyStop'
import PositionsTable from './components/PositionsTable'
import ActivityFeed from './components/ActivityFeed'
import LatencyPanel from './components/LatencyPanel'
import FailsafePanel from './components/FailsafePanel'
import CircuitBreakerAlarm from './components/CircuitBreakerAlarm'
import PortfolioStats from './components/PortfolioStats'
import ErrorBoundary from './components/ErrorBoundary'
import BullpenHealth from './components/BullpenHealth'
import DiscoveryPanel from './components/DiscoveryPanel'
import ConvergenceAlert from './components/ConvergenceAlert'

import MarketsPage from './pages/MarketsPage'
import HistoryPage from './pages/HistoryPage'
import IntelligencePage from './pages/IntelligencePage'
import PerformancePage from './pages/PerformancePage'
import ConfigPage from './pages/ConfigPage'

import type { RiskSummary } from './types'

const EMPTY_RISK: RiskSummary = {
  trading_enabled: true,
  circuit_breaker_tripped: false,
  daily_pnl: 0,
  max_daily_loss_pct: 0.05,
  max_concurrent_positions: 5,
  max_single_order_usdc: 20,
}

export default function App() {
  const { snapshot, connected, lastMessageAt } = useEngineSocket()
  const { viewMode } = useMode()
  const isLive = viewMode === 'live'
  const { activeTab, switchTab, switchByIndex } = useTab()

  const [tradingPaused, setTradingPaused] = useState(false)
  const [cbDismissed, setCbDismissed] = useState(false)

  const { data: livePortfolio } = usePoll(api.livePortfolio, 5_000, isLive)
  const { data: bullpenHealth } = usePoll(api.bullpenHealth, 10_000)
  const { data: bullpenDiscovery } = usePoll(api.bullpenDiscovery, 15_000)
  const { data: bullpenConvergence } = usePoll(api.bullpenConvergence, 5_000)

  const wsPaused = snapshot?.trading_paused ?? tradingPaused
  const risk: RiskSummary = snapshot?.risk ?? EMPTY_RISK
  const portfolio = snapshot?.portfolio
  const activity = snapshot?.recent_activity ?? []
  const positions = portfolio?.open_positions ?? []

  const nav = portfolio?.nav_usdc ?? 0
  const navDelta = isLive
    ? (livePortfolio?.daily_pnl_usdc ?? portfolio?.unrealized_pnl_usdc ?? 0)
    : (portfolio?.unrealized_pnl_usdc ?? 0)
  const navDeltaPct = nav > 0 ? (navDelta / (nav - navDelta || 1)) * 100 : 0

  const realizedPnl = portfolio?.realized_pnl_usdc ?? 0
  const unrealizedPnl = portfolio?.unrealized_pnl_usdc ?? 0
  const feesPaid = portfolio?.fees_paid_usdc ?? 0
  const netPnl = realizedPnl + unrealizedPnl - feesPaid

  const equityCurve = portfolio?.equity_curve ?? []
  const equityTimestamps = portfolio?.equity_timestamps ?? []

  const showCbAlarm = (risk.circuit_breaker_tripped || (risk as unknown as { circuit_breaker?: boolean }).circuit_breaker) && !cbDismissed
  const wsDownSecs = lastMessageAt ? Math.floor((Date.now() - lastMessageAt) / 1000) : 0
  const showWsBanner = !connected && wsDownSecs > 15

  // Keyboard shortcuts
  const handlePause = useCallback(async () => {
    try {
      const res = await api.pause(!wsPaused)
      setTradingPaused(res.trading_paused)
    } catch { /* ignore */ }
  }, [wsPaused])

  useKeyboard({
    onTabSwitch: switchByIndex,
    onPause: handlePause,
  })

  return (
    <div className="h-screen flex flex-col overflow-hidden bg-surface-950 text-slate-100">
      <Header wsConnected={connected} tradingPaused={wsPaused} />
      <TabBar activeTab={activeTab} onSwitch={switchTab} />

      {showWsBanner && (
        <div className="bg-red-900/80 border-b border-red-600 text-red-200 text-xs flex items-center justify-center gap-2 py-1 z-30 shrink-0">
          <span className="w-1.5 h-1.5 rounded-full bg-red-400 animate-pulse" />
          WebSocket disconnected — reconnecting ({wsDownSecs}s ago)
        </div>
      )}

      {showCbAlarm && (
        <CircuitBreakerAlarm risk={risk} onDismiss={() => setCbDismissed(true)} />
      )}

      {/* ── Tab Content ─────────────────────────────────────────── */}

      {activeTab === 'dashboard' && (
        <main className="flex-1 grid grid-cols-1 md:grid-cols-[220px_1fr] xl:grid-cols-[220px_1fr_260px] gap-2 p-2 overflow-hidden min-h-0">

          <aside className="hidden md:flex flex-col gap-2 overflow-y-auto min-h-0">
            <ErrorBoundary label="RiskPanel">
              <RiskPanel risk={risk} />
            </ErrorBoundary>
            <ErrorBoundary label="PortfolioStats">
              <PortfolioStats portfolio={portfolio} />
            </ErrorBoundary>
            {isLive && (
              <ErrorBoundary label="EmergencyStop">
                <EmergencyStop paused={wsPaused} onToggled={(p) => setTradingPaused(p)} />
              </ErrorBoundary>
            )}
          </aside>

          <section className="flex flex-col gap-2 overflow-y-auto min-h-0">
            <ErrorBoundary label="NavCard">
              <NavCard
                nav={nav}
                navDelta={navDelta}
                navDeltaPct={navDeltaPct}
                netPnl={netPnl}
                feesPaid={feesPaid}
                equityCurve={equityCurve}
                equityTimestamps={equityTimestamps}
                portfolio={portfolio}
              />
            </ErrorBoundary>
            <ErrorBoundary label="PositionsTable">
              <PositionsTable
                positions={positions}
                loading={!snapshot && connected}
                isLive={isLive}
              />
            </ErrorBoundary>
            <ErrorBoundary label="ActivityFeed">
              <ActivityFeed wsEntries={activity} />
            </ErrorBoundary>
          </section>

          <aside className="hidden xl:flex flex-col gap-2 overflow-y-auto min-h-0">
            <ErrorBoundary label="LatencyPanel">
              <LatencyPanel />
            </ErrorBoundary>
            <ErrorBoundary label="FailsafePanel">
              <FailsafePanel />
            </ErrorBoundary>
            <ErrorBoundary label="BullpenHealth">
              <BullpenHealth health={bullpenHealth} />
            </ErrorBoundary>
            <ErrorBoundary label="DiscoveryPanel">
              <DiscoveryPanel discovery={bullpenDiscovery} />
            </ErrorBoundary>
            <ErrorBoundary label="ConvergenceAlert">
              <ConvergenceAlert convergence={bullpenConvergence} />
            </ErrorBoundary>
          </aside>

        </main>
      )}

      {activeTab === 'markets' && <MarketsPage />}
      {activeTab === 'history' && <HistoryPage />}
      {activeTab === 'intelligence' && <IntelligencePage />}
      {activeTab === 'performance' && <PerformancePage portfolio={portfolio} positions={positions} />}
      {activeTab === 'config' && <ConfigPage risk={risk} />}

      <StatusBar />
    </div>
  )
}