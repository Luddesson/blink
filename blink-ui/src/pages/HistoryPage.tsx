import { usePoll } from '../hooks/usePoll'
import { api } from '../lib/api'
import ErrorBoundary from '../components/ErrorBoundary'
import TradeHistory from '../components/TradeHistory'
import ExecutionScorecard from '../components/ExecutionScorecard'
import PnlAttribution from '../components/PnlAttribution'
import DrawdownTracker from '../components/DrawdownTracker'
import type { ClosedTrade } from '../types'
import { useState, useEffect } from 'react'

export default function HistoryPage() {
  const { data: history } = usePoll(() => api.history(1, 200), 10_000)
  const [allTrades, setAllTrades] = useState<ClosedTrade[]>([])

  useEffect(() => {
    if (history?.trades) setAllTrades(history.trades)
  }, [history])

  return (
    <div className="flex-1 flex flex-col gap-2 p-2 overflow-y-auto min-h-0">
      <ErrorBoundary label="TradeHistory">
        <TradeHistory />
      </ErrorBoundary>

      <ErrorBoundary label="ExecutionScorecard">
        <ExecutionScorecard trades={allTrades} />
      </ErrorBoundary>

      <div className="grid grid-cols-2 gap-2">
        <ErrorBoundary label="PnlAttribution">
          <PnlAttribution trades={allTrades} />
        </ErrorBoundary>
        <ErrorBoundary label="DrawdownTracker">
          <DrawdownTracker equityCurve={[]} />
        </ErrorBoundary>
      </div>
    </div>
  )
}
