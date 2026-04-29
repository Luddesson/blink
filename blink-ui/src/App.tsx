import { lazy, Suspense, useCallback, useState, useEffect, useMemo } from 'react'
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
import WalletPositionsTable from './components/WalletPositionsTable'
import ActivityFeed from './components/ActivityFeed'
import LatencyPanel from './components/LatencyPanel'
import FailsafePanel from './components/FailsafePanel'
import CircuitBreakerAlarm from './components/CircuitBreakerAlarm'
import PortfolioStats from './components/PortfolioStats'
import ErrorBoundary from './components/ErrorBoundary'
import BullpenHealth from './components/BullpenHealth'
import DiscoveryPanel from './components/DiscoveryPanel'
import ConvergencePanel from './components/ConvergencePanel'
import ShaderBackground from './components/aurora/ShaderBackground'
import CommandPalette from './components/CommandPalette'
import HelpSheet from './components/HelpSheet'
import { ToastProvider } from './components/ui'
import { audio } from './lib/audioContext'

import type { PortfolioSummary, RiskSummary } from './types'

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
  const risk: RiskSummary = useMemo(() => {
    if (isLive && livePortfolio) {
      return {
        ...(snapshot?.risk ?? EMPTY_RISK),
        trading_enabled: livePortfolio.trading_enabled,
        circuit_breaker_tripped: livePortfolio.circuit_breaker_tripped,
        circuit_breaker: livePortfolio.circuit_breaker_tripped,
        daily_pnl: livePortfolio.daily_pnl_usdc,
        max_daily_loss_pct: livePortfolio.max_daily_loss_pct,
      }
    }
    return snapshot?.risk ?? EMPTY_RISK
  }, [isLive, livePortfolio, snapshot?.risk])
  const snapshotPortfolio = snapshot?.portfolio
  const portfolio = useMemo<PortfolioSummary | undefined>(() => {
    if (isLive && livePortfolio && !snapshotPortfolio) {
      const exchangeVerified = livePortfolio.exchange_positions_verified === true
      const walletNavVerified = livePortfolio.wallet_truth_verified === true
        || (exchangeVerified && livePortfolio.onchain_cash_verified === true)
      const walletNav = walletNavVerified
        ? (livePortfolio.wallet_nav_usdc ?? livePortfolio.nav_usdc ?? 0)
        : 0
      return {
        cash_usdc: livePortfolio.onchain_cash_verified ? (livePortfolio.cash_usdc ?? 0) : 0,
        nav_usdc: walletNav,
        blink_cash_usdc: livePortfolio.blink_cash_usdc,
        blink_nav_usdc: livePortfolio.blink_nav_usdc,
        wallet_nav_usdc: walletNavVerified ? walletNav : undefined,
        invested_usdc: exchangeVerified ? (livePortfolio.invested_usdc ?? 0) : 0,
        unrealized_pnl_usdc: exchangeVerified ? (livePortfolio.unrealized_pnl_usdc ?? 0) : 0,
        realized_pnl_usdc: livePortfolio.realized_pnl_usdc ?? 0,
        fees_paid_usdc: livePortfolio.fees_paid_usdc ?? 0,
        cash_source: livePortfolio.cash_source,
        balance_source: livePortfolio.balance_source,
        exchange_position_value_usdc: livePortfolio.exchange_position_value_usdc,
        external_position_value_usdc: livePortfolio.external_position_value_usdc,
        wallet_position_value_usdc: exchangeVerified ? (livePortfolio.wallet_position_value_usdc ?? undefined) : undefined,
        wallet_position_initial_value_usdc: exchangeVerified ? (livePortfolio.wallet_position_initial_value_usdc ?? undefined) : undefined,
        wallet_open_pnl_usdc: exchangeVerified ? (livePortfolio.wallet_open_pnl_usdc ?? undefined) : undefined,
        wallet_unrealized_pnl_usdc: exchangeVerified ? (livePortfolio.wallet_unrealized_pnl_usdc ?? undefined) : undefined,
        wallet_pnl_source: exchangeVerified ? livePortfolio.wallet_pnl_source : 'unverified',
        pnl_source: exchangeVerified ? livePortfolio.pnl_source : 'unverified',
        exchange_positions_count: exchangeVerified ? (livePortfolio.exchange_positions_count ?? 0) : 0,
        exchange_positions_preview: exchangeVerified ? (livePortfolio.exchange_positions_preview ?? []) : [],
        wallet_positions_count: exchangeVerified ? (livePortfolio.wallet_positions_count ?? 0) : 0,
        reality_status: livePortfolio.reality_status,
        reality_issues: livePortfolio.reality_issues,
        truth_checked_at_ms: livePortfolio.truth_checked_at_ms,
        exchange_positions_verified: livePortfolio.exchange_positions_verified,
        onchain_cash_verified: livePortfolio.onchain_cash_verified,
        wallet_truth_verified: livePortfolio.wallet_truth_verified,
        blink_wallet_truth_last_sync_ms: livePortfolio.blink_wallet_truth_last_sync_ms,
        blink_wallet_truth_sync_age_ms: livePortfolio.blink_wallet_truth_sync_age_ms,
        external_only_positions_count: livePortfolio.external_only_positions_count,
        local_only_positions_count: livePortfolio.local_only_positions_count,
        local_open_positions_count: livePortfolio.local_open_positions_count,
        open_positions: exchangeVerified ? (livePortfolio.open_positions ?? []) : [],
        closed_trades_count: 0,
        total_signals: 0,
        filled_orders: livePortfolio.confirmed_fills ?? 0,
        skipped_orders: livePortfolio.no_fills ?? 0,
        aborted_orders: livePortfolio.stale_orders ?? 0,
        fill_rate_pct: livePortfolio.confirmation_rate_pct ?? 0,
        reject_rate_pct: undefined,
        equity_curve: [],
        equity_timestamps: [],
        win_rate_pct: 0,
        uptime_secs: livePortfolio.uptime_secs ?? 0,
      }
    }
    if (!snapshotPortfolio) return undefined
    if (!isLive) return snapshotPortfolio
    if (!livePortfolio) {
      return {
        ...snapshotPortfolio,
        cash_usdc: 0,
        nav_usdc: 0,
        wallet_nav_usdc: undefined,
        invested_usdc: 0,
        unrealized_pnl_usdc: 0,
        wallet_position_value_usdc: undefined,
        wallet_position_initial_value_usdc: undefined,
        wallet_open_pnl_usdc: undefined,
        wallet_unrealized_pnl_usdc: undefined,
        wallet_pnl_source: 'unverified',
        pnl_source: 'unverified',
        exchange_positions_count: 0,
        exchange_positions_preview: [],
        wallet_positions_count: 0,
        reality_status: 'unverified',
        reality_issues: ['live_wallet_truth_not_loaded'],
        exchange_positions_verified: false,
        onchain_cash_verified: false,
        wallet_truth_verified: false,
        open_positions: [],
        equity_curve: [],
        equity_timestamps: [],
      }
    }

    const exchangeVerified = livePortfolio.exchange_positions_verified === true
    const walletNavVerified = livePortfolio.wallet_truth_verified === true
      || (exchangeVerified && livePortfolio.onchain_cash_verified === true)
    const walletNav = walletNavVerified
      ? (livePortfolio.wallet_nav_usdc ?? livePortfolio.nav_usdc ?? 0)
      : 0

    return {
      ...snapshotPortfolio,
      cash_usdc: livePortfolio.onchain_cash_verified ? (livePortfolio.cash_usdc ?? 0) : 0,
      nav_usdc: walletNav,
      blink_cash_usdc: livePortfolio.blink_cash_usdc ?? snapshotPortfolio.blink_cash_usdc,
      blink_nav_usdc: livePortfolio.blink_nav_usdc ?? snapshotPortfolio.blink_nav_usdc,
      wallet_nav_usdc: walletNavVerified ? walletNav : undefined,
      invested_usdc: exchangeVerified ? (livePortfolio.invested_usdc ?? 0) : 0,
      unrealized_pnl_usdc: exchangeVerified ? (livePortfolio.unrealized_pnl_usdc ?? 0) : 0,
      realized_pnl_usdc: livePortfolio.realized_pnl_usdc ?? snapshotPortfolio.realized_pnl_usdc,
      fees_paid_usdc: livePortfolio.fees_paid_usdc ?? snapshotPortfolio.fees_paid_usdc,
      cash_source: livePortfolio.cash_source ?? snapshotPortfolio.cash_source,
      balance_source: livePortfolio.balance_source ?? snapshotPortfolio.balance_source,
      exchange_position_value_usdc: livePortfolio.exchange_position_value_usdc ?? snapshotPortfolio.exchange_position_value_usdc,
      external_position_value_usdc: livePortfolio.external_position_value_usdc ?? snapshotPortfolio.external_position_value_usdc,
      wallet_position_value_usdc: exchangeVerified ? (livePortfolio.wallet_position_value_usdc ?? undefined) : undefined,
      wallet_position_initial_value_usdc: exchangeVerified ? (livePortfolio.wallet_position_initial_value_usdc ?? undefined) : undefined,
      wallet_open_pnl_usdc: exchangeVerified ? (livePortfolio.wallet_open_pnl_usdc ?? undefined) : undefined,
      wallet_unrealized_pnl_usdc: exchangeVerified ? (livePortfolio.wallet_unrealized_pnl_usdc ?? undefined) : undefined,
      wallet_pnl_source: exchangeVerified ? livePortfolio.wallet_pnl_source : 'unverified',
      pnl_source: exchangeVerified ? livePortfolio.pnl_source : 'unverified',
      exchange_positions_count: exchangeVerified ? (livePortfolio.exchange_positions_count ?? 0) : 0,
      exchange_positions_preview: exchangeVerified ? (livePortfolio.exchange_positions_preview ?? []) : [],
      wallet_positions_count: exchangeVerified ? (livePortfolio.wallet_positions_count ?? 0) : 0,
      reality_status: livePortfolio.reality_status ?? snapshotPortfolio.reality_status,
      reality_issues: livePortfolio.reality_issues ?? snapshotPortfolio.reality_issues,
      truth_checked_at_ms: livePortfolio.truth_checked_at_ms ?? snapshotPortfolio.truth_checked_at_ms,
      exchange_positions_verified: livePortfolio.exchange_positions_verified ?? snapshotPortfolio.exchange_positions_verified,
      onchain_cash_verified: livePortfolio.onchain_cash_verified ?? snapshotPortfolio.onchain_cash_verified,
      wallet_truth_verified: livePortfolio.wallet_truth_verified ?? snapshotPortfolio.wallet_truth_verified,
      blink_wallet_truth_last_sync_ms: livePortfolio.blink_wallet_truth_last_sync_ms ?? snapshotPortfolio.blink_wallet_truth_last_sync_ms,
      blink_wallet_truth_sync_age_ms: livePortfolio.blink_wallet_truth_sync_age_ms ?? snapshotPortfolio.blink_wallet_truth_sync_age_ms,
      external_only_positions_count: livePortfolio.external_only_positions_count ?? snapshotPortfolio.external_only_positions_count,
      local_only_positions_count: livePortfolio.local_only_positions_count ?? snapshotPortfolio.local_only_positions_count,
      local_open_positions_count: livePortfolio.local_open_positions_count ?? snapshotPortfolio.local_open_positions_count,
      open_positions: exchangeVerified ? (livePortfolio.open_positions ?? []) : [],
      uptime_secs: livePortfolio.uptime_secs ?? snapshotPortfolio.uptime_secs,
    }
  }, [isLive, livePortfolio, snapshotPortfolio])
  const activity = useMemo(() => snapshot?.recent_activity ?? [], [snapshot?.recent_activity])
  const positions = portfolio?.open_positions ?? []
  const walletPositionsVerified = isLive && portfolio?.exchange_positions_verified === true
  const walletNavVerified = walletPositionsVerified && portfolio?.onchain_cash_verified === true
  const walletPositions = isLive && walletPositionsVerified ? (portfolio?.exchange_positions_preview ?? []) : []
  const walletPositionCount = isLive ? (portfolio?.exchange_positions_count ?? walletPositions.length) : 0
  const walletPositionValue = isLive
    ? (walletPositionsVerified ? (portfolio?.exchange_position_value_usdc ?? portfolio?.external_position_value_usdc ?? 0) : 0)
    : 0

  const nav = isLive
    ? (walletNavVerified ? (portfolio?.wallet_nav_usdc ?? portfolio?.nav_usdc ?? 0) : 0)
    : (portfolio?.nav_usdc ?? 0)
  const walletOpenPnl = walletPositionsVerified
    ? (portfolio?.wallet_open_pnl_usdc
      ?? portfolio?.wallet_unrealized_pnl_usdc
      ?? portfolio?.unrealized_pnl_usdc
      ?? 0)
    : 0
  const walletPositionBasis = walletPositionsVerified
    ? (portfolio?.wallet_position_initial_value_usdc ?? portfolio?.invested_usdc ?? 0)
    : 0
  const navDelta = isLive
    ? walletOpenPnl
    : (portfolio?.unrealized_pnl_usdc ?? 0)
  const navDeltaPct = isLive
    ? (walletPositionBasis > 0 ? (navDelta / walletPositionBasis) * 100 : 0)
    : (nav > 0 ? (navDelta / (nav - navDelta || 1)) * 100 : 0)

  const realizedPnl = portfolio?.realized_pnl_usdc ?? 0
  const unrealizedPnl = portfolio?.unrealized_pnl_usdc ?? 0
  const feesPaid = portfolio?.fees_paid_usdc ?? 0
  const netPnl = isLive ? walletOpenPnl : realizedPnl + unrealizedPnl - feesPaid

  const equityCurve = portfolio?.equity_curve ?? []
  const equityTimestamps = portfolio?.equity_timestamps ?? []

  const showCbAlarm = (risk.circuit_breaker_tripped || (risk as unknown as { circuit_breaker?: boolean }).circuit_breaker) && !cbDismissed
  const wsDownSecs = lastMessageAt ? Math.floor((Date.now() - lastMessageAt) / 1000) : 0
  const showWsBanner = !connected && wsDownSecs > 15
  const realityStatus = isLive ? (portfolio?.reality_status ?? livePortfolio?.reality_status) : undefined
  const realityIssues = isLive ? (portfolio?.reality_issues ?? livePortfolio?.reality_issues ?? []) : []
  const showRealityBanner = isLive && realityStatus !== undefined && realityStatus !== 'matched'

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

  // Initialize AudioEngine on first click
  useEffect(() => {
    const initAudio = () => {
      audio.init()
      window.removeEventListener('click', initAudio)
      window.removeEventListener('keydown', initAudio)
    }
    window.addEventListener('click', initAudio)
    window.addEventListener('keydown', initAudio)
    return () => {
      window.removeEventListener('click', initAudio)
      window.removeEventListener('keydown', initAudio)
    }
  }, [])

  // Play sound on new activity
  useEffect(() => {
    if (activity.length > 0) {
      const latest = activity[0]
      if (latest.kind === 'ORDER_FILLED') {
        const isSell = latest.message.toLowerCase().includes('sell')
        audio.playTrade(isSell ? 'sell' : 'buy', 0.6)
      } else if (latest.kind === 'REJECTED' || latest.kind === 'ERROR') {
        audio.playAlert()
      }
    }
  }, [activity])

  return (
    <ToastProvider>
      <ShaderBackground />
      <div className="relative flex h-[100dvh] min-h-[100dvh] flex-col overflow-hidden text-[color:var(--color-text-primary)]">
        <Header
          wsConnected={connected}
          tradingPaused={wsPaused}
          nav={nav}
          navLabel={isLive ? 'Wallet NAV' : 'NAV'}
          navDelta={navDelta}
          navDeltaPct={navDeltaPct}
          positionCount={positions.length}
          walletPositionCount={walletPositionCount}
          realityStatus={realityStatus}
          truthCheckedAtMs={portfolio?.truth_checked_at_ms ?? livePortfolio?.truth_checked_at_ms}
          blinkWalletTruthSyncAgeMs={portfolio?.blink_wallet_truth_sync_age_ms ?? livePortfolio?.blink_wallet_truth_sync_age_ms}
        />
        <TabBar activeTab={activeTab} onSwitch={switchTab} />

        <AnimatePresence>
          {showWsBanner && (
            <motion.div
              initial={{ height: 0, opacity: 0 }}
              animate={{ height: 'auto', opacity: 1 }}
              exit={{ height: 0, opacity: 0 }}
              className="overflow-hidden border-b border-[color:var(--color-bear-500)/0.4] bg-[color:var(--color-bear-600)/0.35] backdrop-blur-sm text-[color:var(--color-bear-300)] text-xs flex items-center justify-center gap-2 py-1.5 z-30 shrink-0"
            >
              <span className="w-1.5 h-1.5 rounded-full bg-[color:var(--color-bear-500)] animate-pulse" />
              WebSocket disconnected — reconnecting ({wsDownSecs}s ago)
            </motion.div>
          )}
        </AnimatePresence>

        <AnimatePresence>
          {showRealityBanner && (
            <motion.div
              initial={{ height: 0, opacity: 0 }}
              animate={{ height: 'auto', opacity: 1 }}
              exit={{ height: 0, opacity: 0 }}
              className="overflow-hidden border-b border-amber-500/40 bg-amber-950/45 px-3 py-1.5 text-center text-xs font-semibold text-amber-100 backdrop-blur-sm"
            >
              Exchange reality {realityStatus} · {realityIssues.length > 0 ? realityIssues.join(', ') : 'truth check incomplete'}
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
              <main className="flex-1 flex flex-col gap-4 overflow-y-auto p-4 md:grid md:grid-cols-[240px_1fr_300px] md:overflow-hidden">
                {/* Left Sidebar */}
                <aside className="order-2 flex flex-col gap-4 md:min-h-0 md:overflow-y-auto md:pr-1 xl:order-1">
                  <ErrorBoundary label="RiskPanel">
                    <RiskPanel risk={risk} />
                  </ErrorBoundary>
                  <ErrorBoundary label="PortfolioStats">
                    <PortfolioStats portfolio={portfolio} />
                  </ErrorBoundary>
                  <ErrorBoundary label="BullpenHealth">
                    <BullpenHealth health={bullpenHealth} />
                  </ErrorBoundary>
                </aside>

                {/* Main Content: Forced Vertical Stacking */}
                <section className="order-1 flex min-h-0 flex-col gap-4 md:order-2 md:overflow-y-auto">
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
                  
                  {/* TWO ROWS: Positions on top, Activity below */}
                  <div className="flex flex-col gap-4">
                    <ErrorBoundary label="PositionsTable">
                      <PositionsTable positions={positions} loading={!snapshot && connected} isLive={isLive} />
                    </ErrorBoundary>
                    <ErrorBoundary label="WalletPositionsTable">
                      <WalletPositionsTable
                        positions={walletPositions}
                        totalCount={walletPositionCount}
                        totalValue={walletPositionValue}
                      />
                    </ErrorBoundary>
                    <ErrorBoundary label="ActivityFeed">
                      <ActivityFeed wsEntries={activity} />
                    </ErrorBoundary>
                  </div>
                </section>

                {/* Right Sidebar */}
                <aside className="order-3 flex flex-col gap-4 md:min-h-0 xl:overflow-y-auto xl:pl-1">
                  <ErrorBoundary label="AuroraMetricStrip">
                    <AuroraMetricStrip
                      nav={nav}
                      realized={isLive ? 0 : realizedPnl}
                      unrealized={isLive ? walletOpenPnl : unrealizedPnl}
                      fees={isLive ? 0 : feesPaid}
                      winRate={isLive ? 0 : (portfolio?.win_rate_pct ?? 0)}
                      closedTrades={isLive ? walletPositionCount : (portfolio?.closed_trades_count ?? 0)}
                      walletPositionValue={portfolio?.wallet_position_value_usdc ?? walletPositionValue}
                      walletPositionBasis={walletPositionBasis}
                      walletPositions={walletPositionCount}
                      walletTruthVerified={walletPositionsVerified}
                      walletNavVerified={walletNavVerified}
                      realityStatus={portfolio?.reality_status}
                    />
                  </ErrorBoundary>
                  <ErrorBoundary label="LatencyPanel"><LatencyPanel /></ErrorBoundary>
                  <ErrorBoundary label="FailsafePanel"><FailsafePanel /></ErrorBoundary>
                  <ErrorBoundary label="DiscoveryPanel"><DiscoveryPanel discovery={bullpenDiscovery} /></ErrorBoundary>
                  <ErrorBoundary label="ConvergencePanel"><ConvergencePanel convergence={bullpenConvergence} variant="compact" /></ErrorBoundary>
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
