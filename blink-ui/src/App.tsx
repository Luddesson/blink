import { useState } from 'react'
import { useEngineSocket } from './hooks/useEngineSocket'
import { usePoll } from './hooks/usePoll'
import { useMode } from './hooks/useMode'
import { api } from './lib/api'

import Header from './components/Header'
import NavCard from './components/NavCard'
import RiskPanel from './components/RiskPanel'
import EmergencyStop from './components/EmergencyStop'
import PositionsTable from './components/PositionsTable'
import TradeHistory from './components/TradeHistory'
import ActivityFeed from './components/ActivityFeed'
import LatencyPanel from './components/LatencyPanel'
import FailsafePanel from './components/FailsafePanel'
import CircuitBreakerAlarm from './components/CircuitBreakerAlarm'
import PortfolioStats from './components/PortfolioStats'
import ErrorBoundary from './components/ErrorBoundary'

import type { RiskSummary } from './types'

const EMPTY_RISK: RiskSummary = {
  trading_enabled: true,
  circuit_breaker_tripped: false,
  daily_pnl: 0,
  max_daily_loss_pct: 0.05,
  max_concurrent_positions: 5,
  max_single_order_usdc: 20,
}

// Paper run ends 2026-04-13T09:34:35Z
const PAPER_RUN_END = new Date('2026-04-13T09:34:35Z').getTime()

function PaperRunCountdown() {
  const now = Date.now()
  const msLeft = PAPER_RUN_END - now
  if (msLeft <= 0) return <span className="badge badge-ok">Paper run complete</span>
  const days = Math.floor(msLeft / 86_400_000)
  const hours = Math.floor((msLeft % 86_400_000) / 3_600_000)
  return (
    <span className="badge badge-paper">
      Paper run: {days}d {hours}h left
    </span>
  )
}

export default function App() {
  const { snapshot, connected, lastMessageAt } = useEngineSocket()
  const { viewMode } = useMode()
  const isLive = viewMode === 'live'

  const [tradingPaused, setTradingPaused] = useState(false)
  const [cbDismissed, setCbDismissed] = useState(false)

  // Only poll live portfolio when in live view
  const { data: livePortfolio } = usePoll(api.livePortfolio, 5_000, isLive)

  const wsPaused = snapshot?.trading_paused ?? tradingPaused

  const risk: RiskSummary = snapshot?.risk ?? EMPTY_RISK
  const portfolioSummary = snapshot?.portfolio
  const activity = snapshot?.recent_activity ?? []

  // Positions come from WS snapshot (open_positions array)
  const positions = portfolioSummary?.open_positions ?? []
  const wsLoading = !snapshot && connected

  const nav = isLive
    ? (portfolioSummary?.nav_usdc ?? 0)
    : (portfolioSummary?.nav_usdc ?? 0)

  const navDelta = isLive
    ? (livePortfolio?.daily_pnl_usdc ?? portfolioSummary?.unrealized_pnl_usdc ?? 0)
    : (portfolioSummary?.unrealized_pnl_usdc ?? 0)

  const navDeltaPct = nav > 0 ? (navDelta / (nav - navDelta || 1)) * 100 : 0

  const equityCurve = portfolioSummary?.equity_curve ?? []
  const equityTimestamps = portfolioSummary?.equity_timestamps ?? []

  const showCbAlarm = (risk.circuit_breaker_tripped || (risk as unknown as { circuit_breaker?: boolean }).circuit_breaker) && !cbDismissed

  // WS disconnection banner: show if >15s since last message and was previously connected
  const wsDownSecs = lastMessageAt ? Math.floor((Date.now() - lastMessageAt) / 1000) : 0
  const showWsBanner = !connected && wsDownSecs > 15

  return (
    <div className="min-h-screen bg-surface-950 text-slate-100">
      <Header wsConnected={connected} tradingPaused={wsPaused} />

      {/* WS disconnection banner */}
      {Boolean(showWsBanner) && (
        <div className="bg-red-900/80 border-b border-red-600 text-red-200 text-xs flex items-center justify-center gap-2 py-1.5 sticky top-12 z-30">
          <span className="w-2 h-2 rounded-full bg-red-400 animate-pulse" />
          WebSocket disconnected — attempting to reconnect ({wsDownSecs}s ago)
        </div>
      )}

      {/* Circuit breaker full-screen overlay */}
      {showCbAlarm && (
        <CircuitBreakerAlarm
          risk={risk}
          onDismiss={() => setCbDismissed(true)}
        />
      )}

      {/* Main grid */}
      <main className="grid grid-cols-[240px_1fr_240px] gap-3 p-4 max-w-[1600px] mx-auto">
        {/* Left column */}
        <div className="flex flex-col gap-3">
          <ErrorBoundary label="NavCard">
            <NavCard
              nav={nav}
              navDelta={navDelta}
              navDeltaPct={navDeltaPct}
              equityCurve={equityCurve}
              equityTimestamps={equityTimestamps}
            />
          </ErrorBoundary>
          <ErrorBoundary label="RiskPanel">
            <RiskPanel risk={risk} />
          </ErrorBoundary>
          {!isLive && (
            <div className="flex justify-center">
              <PaperRunCountdown />
            </div>
          )}
          {isLive && (
            <EmergencyStop
              paused={wsPaused}
              onToggled={(p) => setTradingPaused(p)}
            />
          )}
        </div>

        {/* Center column */}
        <div className="flex flex-col gap-3 min-w-0">
          <ErrorBoundary label="PositionsTable">
            <PositionsTable
              positions={positions}
              loading={wsLoading}
              isLive={isLive}
            />
          </ErrorBoundary>
          <ErrorBoundary label="TradeHistory">
            <TradeHistory />
          </ErrorBoundary>
        </div>

        {/* Right column */}
        <div className="flex flex-col gap-3">
          <ErrorBoundary label="PortfolioStats">
            <PortfolioStats portfolio={portfolioSummary} />
          </ErrorBoundary>
          <ErrorBoundary label="ActivityFeed">
            <ActivityFeed wsEntries={activity} />
          </ErrorBoundary>
          <ErrorBoundary label="LatencyPanel">
            <LatencyPanel />
          </ErrorBoundary>
          <ErrorBoundary label="FailsafePanel">
            <FailsafePanel />
          </ErrorBoundary>
        </div>
      </main>

      {/* Footer */}
      <footer className="text-center text-xs text-slate-700 py-3 border-t border-surface-700 mt-2">
        Blink HFT Engine — {snapshot ? `${snapshot.messages_total} msgs processed` : 'connecting…'}
      </footer>
    </div>
  )
}
