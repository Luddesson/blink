import { lazy, Suspense, useCallback, useState } from 'react'
import { motion, AnimatePresence } from 'motion/react'
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
import FloatingEmergencyStop from './components/FloatingEmergencyStop'
import AuroraMetricStrip from './components/AuroraMetricStrip'
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
import AuroraBackground from './components/aurora/AuroraBackground'
import CommandPalette from './components/CommandPalette'
import HelpSheet from './components/HelpSheet'
import { ToastProvider } from './components/ui'

import type { RiskSummary } from './types'

const MarketsPage = lazy(() => import('./pages/MarketsPage'))
const HistoryPage = lazy(() => import('./pages/HistoryPage'))
const BullpenPage = lazy(() => import('./pages/BullpenPage'))
const PerformancePage = lazy(() => import('./pages/PerformancePage'))
const ProjectInventoryPage = lazy(() => import('./pages/ProjectInventoryPage'))
const ConfigPage = lazy(() => import('./pages/ConfigPage'))
const AlphaPage = lazy(() => import('./pages/AlphaPage'))

const EMPTY_RISK: RiskSummary = {
  trading_enabled: true,
  circuit_breaker_tripped: false,
  daily_pnl: 0,
  max_daily_loss_pct: 0.05,
  max_concurrent_positions: 5,
  max_single_order_usdc: 20,
}

const pageTransition = {
  initial: { opacity: 0, y: 8 },
  animate: { opacity: 1, y: 0 },
  exit: { opacity: 0, y: -8 },
  transition: { duration: 0.22, ease: [0.2, 0, 0, 1] as [number, number, number, number] },
}

function PageFallback() {
  return (
    <div className="flex-1 flex items-center justify-center p-4 text-xs text-[color:var(--color-text-muted)]">
      Loading view...
    </div>
  )
}

export default function App() {
  const { snapshot, connected, lastMessageAt } = useEngineSocket()
  const { viewMode } = useMode()
  const isLive = viewMode === 'live'
  const { activeTab, switchTab, switchByIndex } = useTab()

  const [tradingPaused, setTradingPaused] = useState(false)
  const [cbDismissed, setCbDismissed] = useState(false)
  const [paletteOpen, setPaletteOpen] = useState(false)
  const [helpOpen, setHelpOpen] = useState(false)

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

  const handlePause = useCallback(async () => {
    try {
      const res = await api.pause(!wsPaused)
      setTradingPaused(res.trading_paused)
    } catch { /* ignore */ }
  }, [wsPaused])

  useKeyboard({
    onTabSwitch: switchByIndex,
    onPause: handlePause,
    onPalette: () => setPaletteOpen((v) => !v),
    onHelp: () => setHelpOpen((v) => !v),
  })

  return (
    <ToastProvider>
      <AuroraBackground intensity={isLive ? 'intense' : 'normal'} />
      <div className="relative flex h-[100dvh] min-h-[100dvh] flex-col overflow-hidden text-[color:var(--color-text-primary)]">
        <Header
          wsConnected={connected}
          tradingPaused={wsPaused}
          nav={nav}
          navDelta={navDelta}
          navDeltaPct={navDeltaPct}
          positionCount={positions.length}
        />
        <TabBar activeTab={activeTab} onSwitch={switchTab} />

        <AnimatePresence>
          {showWsBanner && (
            <motion.div
              initial={{ height: 0, opacity: 0 }}
              animate={{ height: 'auto', opacity: 1 }}
              exit={{ height: 0, opacity: 0 }}
              className="overflow-hidden border-b border-[color:oklch(0.65_0.24_25/0.4)] bg-[color:oklch(0.30_0.12_25/0.35)] backdrop-blur-sm text-[color:var(--color-bear-300)] text-xs flex items-center justify-center gap-2 py-1.5 z-30 shrink-0"
            >
              <span className="w-1.5 h-1.5 rounded-full bg-[color:var(--color-bear-500)] animate-pulse" />
              WebSocket disconnected — reconnecting ({wsDownSecs}s ago)
            </motion.div>
          )}
        </AnimatePresence>

        {showCbAlarm && (
          <CircuitBreakerAlarm risk={risk} onDismiss={() => setCbDismissed(true)} />
        )}

        {/* Page content — animated transitions */}
        <AnimatePresence mode="wait">
          <motion.div key={activeTab} {...pageTransition} className="flex-1 flex flex-col overflow-hidden min-h-0">
            {activeTab === 'dashboard' && (
              <main className="flex-1 grid min-h-0 grid-cols-1 gap-2.5 overflow-y-auto p-2.5 md:overflow-hidden md:grid-cols-[230px_1fr] xl:grid-cols-[230px_1fr_270px]">
                <aside className="order-2 flex flex-col gap-2.5 md:min-h-0 md:overflow-y-auto md:pr-1 xl:order-1">
                  <ErrorBoundary label="RiskPanel">
                    <RiskPanel risk={risk} />
                  </ErrorBoundary>
                  <ErrorBoundary label="PortfolioStats">
                    <PortfolioStats portfolio={portfolio} />
                  </ErrorBoundary>
                </aside>

                <section className="order-1 flex min-h-0 flex-col gap-2.5 md:order-2 md:overflow-y-auto">
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
                  <ErrorBoundary label="AuroraMetricStrip">
                    <AuroraMetricStrip
                      nav={nav}
                      realized={realizedPnl}
                      unrealized={unrealizedPnl}
                      fees={feesPaid}
                      winRate={portfolio?.win_rate_pct ?? 0}
                      closedTrades={portfolio?.closed_trades_count ?? 0}
                    />
                  </ErrorBoundary>
                  <div className="grid min-h-0 grid-cols-1 gap-2.5 xl:grid-cols-2">
                    <ErrorBoundary label="PositionsTable">
                      <PositionsTable positions={positions} loading={!snapshot && connected} isLive={isLive} />
                    </ErrorBoundary>
                    <ErrorBoundary label="ActivityFeed">
                      <ActivityFeed wsEntries={activity} />
                    </ErrorBoundary>
                  </div>
                </section>

                <aside className="order-3 flex flex-col gap-2.5 md:min-h-0 xl:overflow-y-auto xl:pl-1">
                  <ErrorBoundary label="LatencyPanel"><LatencyPanel /></ErrorBoundary>
                  <ErrorBoundary label="FailsafePanel"><FailsafePanel /></ErrorBoundary>
                  <ErrorBoundary label="BullpenHealth"><BullpenHealth health={bullpenHealth} /></ErrorBoundary>
                  <ErrorBoundary label="DiscoveryPanel"><DiscoveryPanel discovery={bullpenDiscovery} /></ErrorBoundary>
                  <ErrorBoundary label="ConvergenceAlert"><ConvergenceAlert convergence={bullpenConvergence} /></ErrorBoundary>
                </aside>
              </main>
            )}

            {activeTab === 'markets' && (
              <Suspense fallback={<PageFallback />}>
                <MarketsPage />
              </Suspense>
            )}
            {activeTab === 'history' && (
              <Suspense fallback={<PageFallback />}>
                <HistoryPage />
              </Suspense>
            )}
            {activeTab === 'intelligence' && (
              <Suspense fallback={<PageFallback />}>
                <BullpenPage />
              </Suspense>
            )}
            {activeTab === 'performance' && (
              <Suspense fallback={<PageFallback />}>
                <PerformancePage portfolio={portfolio} positions={positions} />
              </Suspense>
            )}
            {activeTab === 'inventory' && (
              <Suspense fallback={<PageFallback />}>
                <ProjectInventoryPage />
              </Suspense>
            )}
            {activeTab === 'config' && (
              <Suspense fallback={<PageFallback />}>
                <ConfigPage risk={risk} connected={connected} />
              </Suspense>
            )}
            {activeTab === 'alpha' && (
              <Suspense fallback={<PageFallback />}>
                <AlphaPage />
              </Suspense>
            )}
          </motion.div>
        </AnimatePresence>

        <StatusBar />

        <FloatingEmergencyStop
          paused={wsPaused}
          onToggled={(p) => setTradingPaused(p)}
          isLive={isLive}
        />

        <CommandPalette
          open={paletteOpen}
          onClose={() => setPaletteOpen(false)}
          onSwitchTab={switchTab}
          onPause={handlePause}
          paused={wsPaused}
        />
        <HelpSheet open={helpOpen} onClose={() => setHelpOpen(false)} />
      </div>
    </ToastProvider>
  )
}
