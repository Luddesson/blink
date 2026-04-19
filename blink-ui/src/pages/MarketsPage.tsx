import { useState, useEffect, useMemo } from 'react'
import { usePoll } from '../hooks/usePoll'
import { api, useMarketMeta } from '../lib/api'
import ErrorBoundary from '../components/ErrorBoundary'
import OrderBookDepth from '../components/OrderBookDepth'
import SpreadIndicator from '../components/SpreadIndicator'
import type { OrderBookResponse } from '../types'

export default function MarketsPage() {
  const { data: books } = usePoll(api.orderbooks, 3_000)
  const { data: latency } = usePoll(api.latency, 5_000)
  const [selectedToken, setSelectedToken] = useState<string | null>(null)
  const [selectedBook, setSelectedBook] = useState<OrderBookResponse | null>(null)
  const [searchQuery, setSearchQuery] = useState('')

  const markets = useMemo(() => books?.orderbooks ?? [], [books])
  const tokenIds = useMemo(() => markets.map(m => m.token_id), [markets])
  const { metadata: marketMeta, loading: metaLoading } = useMarketMeta(tokenIds)

  const listEntry = markets.find((m) => m.token_id === selectedToken) ?? null

  // Filtered markets based on search query
  const filtered = useMemo(() => {
    if (!searchQuery) return markets
    const q = searchQuery.toLowerCase()
    return markets.filter((m) => {
      const meta = marketMeta.get(m.token_id)
      const question = meta?.question ?? m.market_title ?? ''
      return (
        m.token_id.toLowerCase().includes(q) ||
        question.toLowerCase().includes(q)
      )
    })
  }, [markets, searchQuery, marketMeta])

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

  // Auto-select first market on load
  useEffect(() => {
    if (selectedToken === null && filtered.length > 0) {
      setSelectedToken(filtered[0].token_id)
    }
  }, [filtered, selectedToken])

  return (
    <div className="flex-1 grid min-h-0 grid-cols-1 gap-3 overflow-y-auto p-2 xl:grid-cols-[minmax(0,1fr)_320px] xl:overflow-hidden">
      {/* Left panel: Market card grid */}
      <div className="flex min-h-0 flex-col gap-2 overflow-hidden">
        {/* Search */}
        <div className="relative flex-shrink-0">
          <svg
            className="absolute left-2.5 top-1/2 -translate-y-1/2 w-3.5 h-3.5 text-slate-500"
            fill="none"
            stroke="currentColor"
            viewBox="0 0 24 24"
          >
            <path
              strokeLinecap="round"
              strokeLinejoin="round"
              strokeWidth={2}
              d="M21 21l-6-6m2-5a7 7 0 11-14 0 7 7 0 0114 0z"
            />
          </svg>
          <input
            type="text"
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            placeholder="Search markets…"
            className="w-full bg-slate-800 border border-slate-700 rounded px-8 py-1.5 text-slate-200 text-[11px] placeholder:text-slate-600 focus:outline-none focus:border-slate-500"
          />
        </div>

        {/* Market grid — responsive: 1 col mobile, 2 col tablet, 3 col desktop */}
        <div className="flex-1 overflow-y-auto -mx-1 px-1">
          {filtered.length === 0 ? (
            <div className="flex items-center justify-center h-full text-slate-500 text-[11px]">
              {markets.length === 0 ? 'No markets subscribed' : 'No markets match search'}
            </div>
          ) : (
            <div className="grid gap-2 auto-rows-max grid-cols-1 sm:grid-cols-2 lg:grid-cols-3">
              {filtered.map((market) => {
                const meta = marketMeta.get(market.token_id)
                const mid =
                  market.best_bid != null && market.best_ask != null
                    ? (market.best_bid + market.best_ask) / 2
                    : null
                const selected = market.token_id === selectedToken

                return (
                  <MarketCard
                    key={market.token_id}
                    market={market}
                    meta={meta}
                    mid={mid}
                    selected={selected}
                    onSelect={() => setSelectedToken(market.token_id)}
                    loading={metaLoading}
                  />
                )
              })}
            </div>
          )}
        </div>
      </div>

      {/* Right panel: Order book + spread + latency */}
        <div className="grid min-h-0 grid-cols-1 gap-2 sm:grid-cols-2 xl:flex xl:flex-col xl:overflow-y-auto">
        <ErrorBoundary label="SpreadIndicator">
          <div className="card flex-shrink-0">
            <SpreadIndicator
              bestBid={listEntry?.best_bid ?? selectedBook?.best_bid ?? null}
              bestAsk={listEntry?.best_ask ?? selectedBook?.best_ask ?? null}
              spreadBps={listEntry?.spread_bps ?? selectedBook?.spread_bps ?? null}
            />
          </div>
        </ErrorBoundary>

        <ErrorBoundary label="OrderBookDepth">
          <div className="flex-1 min-h-0">
            <OrderBookDepth orderbook={selectedBook} />
          </div>
        </ErrorBoundary>

        {/* Latency row */}
        <ErrorBoundary label="Latency">
          <div className="card flex-shrink-0">
            <div className="text-[10px] font-mono text-slate-400 space-y-1">
              <div className="flex justify-between">
                <span>Min:</span>
                <span className="text-slate-200">{latency?.signal_age?.min_us ? (latency.signal_age.min_us / 1000).toFixed(1) : '-'}ms</span>
              </div>
              <div className="flex justify-between">
                <span>Avg:</span>
                <span className="text-slate-200">{latency?.signal_age?.avg_us ? (latency.signal_age.avg_us / 1000).toFixed(1) : '-'}ms</span>
              </div>
              <div className="flex justify-between">
                <span>P99:</span>
                <span className="text-slate-200">{latency?.signal_age?.p99_us ? (latency.signal_age.p99_us / 1000).toFixed(1) : '-'}ms</span>
              </div>
            </div>
          </div>
        </ErrorBoundary>
      </div>
    </div>
  )
}

interface MarketCardProps {
  market: OrderBookResponse
  meta: import('../lib/api').PolymarketMeta | undefined
  mid: number | null
  selected: boolean
  onSelect: () => void
  loading: boolean
}

function MarketCard({ market, meta, mid, selected, onSelect, loading }: MarketCardProps) {
  const yesPrice = mid ?? 0.5
  const noPrice = 1 - yesPrice

  // Image fallback
  const imageSrc = meta?.image && meta.image.length > 0 ? meta.image : null

  return (
    <button
      onClick={onSelect}
      className={`relative rounded-xl p-3 glass transition-all text-left group ${
        selected
          ? 'ring-2 ring-emerald-400/60 bg-gradient-to-b from-emerald-950/20 to-emerald-950/5'
          : 'hover:from-slate-800/40 hover:to-slate-800/10'
      }`}
    >
      {/* Loading skeleton */}
      {loading && !meta ? (
        <div className="space-y-2">
          <div className="h-6 bg-slate-700/50 rounded animate-pulse" />
          <div className="h-4 bg-slate-700/50 rounded animate-pulse w-2/3" />
          <div className="flex gap-2">
            <div className="h-5 bg-slate-700/50 rounded flex-1 animate-pulse" />
            <div className="h-5 bg-slate-700/50 rounded flex-1 animate-pulse" />
          </div>
        </div>
      ) : (
        <>
          {/* Image + Token ID */}
          <div className="flex items-start gap-2 mb-2">
            {imageSrc ? (
              <img
                src={imageSrc}
                alt={meta?.question ?? ''}
                className="w-8 h-8 rounded-full object-cover flex-shrink-0"
                onError={(e) => {
                  e.currentTarget.style.display = 'none'
                }}
              />
            ) : (
              <div className="w-8 h-8 rounded-full bg-slate-700/60 flex items-center justify-center flex-shrink-0 text-xs">
                🎲
              </div>
            )}
            <span className="text-[9px] font-mono text-slate-400 truncate flex-1">
              {market.token_id.slice(0, 8)}…
            </span>
          </div>

          {/* Question */}
          <h3 className="text-[12px] font-semibold text-slate-100 line-clamp-2 mb-2 leading-tight">
            {meta?.question ?? market.market_title ?? 'Unknown market'}
          </h3>

          {/* Yes/No odds badges */}
          <div className="flex gap-2 mb-2">
            <div className="flex-1 px-2 py-1 rounded-md bg-emerald-950/40 border border-emerald-700/50 text-center">
              <div className="text-[9px] text-emerald-300 font-mono">YES</div>
              <div className="text-[11px] font-semibold text-emerald-400">
                {(yesPrice * 100).toFixed(0)}%
              </div>
            </div>
            <div className="flex-1 px-2 py-1 rounded-md bg-red-950/40 border border-red-700/50 text-center">
              <div className="text-[9px] text-red-300 font-mono">NO</div>
              <div className="text-[11px] font-semibold text-red-400">
                {(noPrice * 100).toFixed(0)}%
              </div>
            </div>
          </div>

          {/* Volume + Spread */}
          <div className="space-y-1 text-[10px] text-slate-400">
            {meta?.volume && meta.volume !== '0' && (
              <div className="flex justify-between">
                <span>Vol (24h):</span>
                <span className="text-slate-300 font-mono">
                  ${parseFloat(meta.volume).toLocaleString(undefined, { maximumFractionDigits: 0 })}
                </span>
              </div>
            )}
            {market.spread_bps != null && (
              <div className="flex justify-between">
                <span>Spread:</span>
                <span
                  className={`font-mono ${
                    market.spread_bps < 50
                      ? 'text-emerald-400'
                      : market.spread_bps <= 200
                        ? 'text-yellow-400'
                        : 'text-red-400'
                  }`}
                >
                  {market.spread_bps}bps
                </span>
              </div>
            )}
          </div>
        </>
      )}
    </button>
  )
}
