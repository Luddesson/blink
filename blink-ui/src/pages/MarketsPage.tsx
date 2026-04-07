import { useState } from 'react'
import { usePoll } from '../hooks/usePoll'
import { api } from '../lib/api'
import ErrorBoundary from '../components/ErrorBoundary'
import MarketList from '../components/MarketList'
import OrderBookDepth from '../components/OrderBookDepth'
import SpreadIndicator from '../components/SpreadIndicator'

export default function MarketsPage() {
  const { data: books } = usePoll(api.orderbooks, 3_000)
  const [selectedToken, setSelectedToken] = useState<string | null>(null)

  const markets = books?.orderbooks ?? []
  const selected = markets.find((m) => m.token_id === selectedToken) ?? null

  return (
    <div className="flex-1 grid grid-cols-[280px_1fr] gap-2 p-2 overflow-hidden min-h-0">
      <ErrorBoundary label="MarketList">
        <MarketList
          markets={markets}
          selectedTokenId={selectedToken}
          onSelect={setSelectedToken}
        />
      </ErrorBoundary>

      <div className="flex flex-col gap-2 overflow-y-auto min-h-0">
        <ErrorBoundary label="SpreadIndicator">
          <SpreadIndicator
            bestBid={selected?.best_bid ?? null}
            bestAsk={selected?.best_ask ?? null}
            spreadBps={selected?.spread_bps ?? null}
          />
        </ErrorBoundary>
        <ErrorBoundary label="OrderBookDepth">
          <OrderBookDepth orderbook={selected} />
        </ErrorBoundary>
      </div>
    </div>
  )
}
