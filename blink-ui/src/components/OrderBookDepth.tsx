import { useMemo, memo } from 'react'
import type { OrderBookResponse } from '../types'

interface Props {
  orderbook: OrderBookResponse | null
}

const MAX_LEVELS = 15

function OrderBookDepth({ orderbook }: Props) {
  const bids = useMemo(
    () => orderbook?.bids?.slice(0, MAX_LEVELS) ?? [],
    [orderbook],
  )
  const asks = useMemo(
    () => orderbook?.asks?.slice(0, MAX_LEVELS) ?? [],
    [orderbook],
  )

  const maxBidSize = useMemo(
    () => Math.max(...bids.map(([, size]) => size), 0) || 1,
    [bids],
  )
  const maxAskSize = useMemo(
    () => Math.max(...asks.map(([, size]) => size), 0) || 1,
    [asks],
  )

  if (!orderbook) {
    return (
      <div className="card flex items-center justify-center h-full">
        <p className="text-slate-500 text-[11px]">Select a market</p>
      </div>
    )
  }

  if (!orderbook.bids || !orderbook.asks) {
    return (
      <div className="card flex items-center justify-center h-full">
        <p className="text-slate-500 text-[11px]">Order book data unavailable</p>
      </div>
    )
  }

  return (
    <div className="card h-full flex flex-col">
      <div className="grid grid-cols-2 gap-4 flex-1 min-h-0">
        {/* Bids */}
        <div className="flex flex-col min-h-0">
          <div className="flex justify-between text-[10px] text-slate-500 uppercase tracking-wider pb-1.5 border-b border-slate-700/50 mb-1">
            <span>Price</span>
            <span>Size</span>
          </div>
          <div className="flex-1 overflow-y-auto">
            {bids.map(([price, size], i) => {
              const pct = (size / maxBidSize) * 100
              const isBest = price === orderbook.best_bid
              return (
                <div key={i} className="relative flex justify-between py-0.5 px-1">
                  <div
                    className="absolute inset-y-0 right-0 bg-emerald-500/20 rounded-sm"
                    style={{ width: `${pct}%` }}
                  />
                  <span
                    className={`relative text-[11px] tabular-nums ${
                      isBest ? 'text-emerald-300 font-semibold' : 'text-emerald-400/80'
                    }`}
                  >
                    {price.toFixed(4)}
                  </span>
                  <span className="relative text-[11px] tabular-nums text-slate-400">
                    {size.toLocaleString()}
                  </span>
                </div>
              )
            })}
          </div>
        </div>

        {/* Asks */}
        <div className="flex flex-col min-h-0">
          <div className="flex justify-between text-[10px] text-slate-500 uppercase tracking-wider pb-1.5 border-b border-slate-700/50 mb-1">
            <span>Price</span>
            <span>Size</span>
          </div>
          <div className="flex-1 overflow-y-auto">
            {asks.map(([price, size], i) => {
              const pct = (size / maxAskSize) * 100
              const isBest = price === orderbook.best_ask
              return (
                <div key={i} className="relative flex justify-between py-0.5 px-1">
                  <div
                    className="absolute inset-y-0 left-0 bg-red-500/20 rounded-sm"
                    style={{ width: `${pct}%` }}
                  />
                  <span
                    className={`relative text-[11px] tabular-nums ${
                      isBest ? 'text-red-300 font-semibold' : 'text-red-400/80'
                    }`}
                  >
                    {price.toFixed(4)}
                  </span>
                  <span className="relative text-[11px] tabular-nums text-slate-400">
                    {size.toLocaleString()}
                  </span>
                </div>
              )
            })}
          </div>
        </div>
      </div>
    </div>
  )
}

export default memo(OrderBookDepth)
