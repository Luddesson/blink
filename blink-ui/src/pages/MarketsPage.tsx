import { useState, useEffect } from 'react'
import { usePoll } from '../hooks/usePoll'
import { api } from '../lib/api'
import ErrorBoundary from '../components/ErrorBoundary'
import MarketList from '../components/MarketList'
import OrderBookDepth from '../components/OrderBookDepth'
import SpreadIndicator from '../components/SpreadIndicator'
import type { OrderBookResponse } from '../types'

export default function MarketsPage() {
  const { data: books } = usePoll(api.orderbooks, 3_000)
  const [selectedToken, setSelectedToken] = useState<string | null>(null)
  const [selectedBook, setSelectedBook] = useState<OrderBookResponse | null>(null)

  const markets = books?.orderbooks ?? []
  const listEntry = markets.find((m) => m.token_id === selectedToken) ?? null

  // Fetch full orderbook (with bids/asks) for the selected market
  useEffect(() => {
    if (!selectedToken) { setSelectedBook(null); return }
    let cancelled = false
    const fetchBook = () => {
      api.orderbook(selectedToken)
        .then((book) => { if (!cancelled) setSelectedBook(book) })
        .catch(() => { if (!cancelled) setSelectedBook(null) })
    }
    fetchBook()
    const timer = setInterval(fetchBook, 3_000)
    return () => { cancelled = true; clearInterval(timer) }
  }, [selectedToken])

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
            bestBid={listEntry?.best_bid ?? selectedBook?.best_bid ?? null}
            bestAsk={listEntry?.best_ask ?? selectedBook?.best_ask ?? null}
            spreadBps={listEntry?.spread_bps ?? selectedBook?.spread_bps ?? null}
          />
        </ErrorBoundary>
        <ErrorBoundary label="OrderBookDepth">
          <OrderBookDepth orderbook={selectedBook} />
        </ErrorBoundary>
      </div>
    </div>
  )
}
