import { useState, useMemo, memo } from 'react'
import { Search } from 'lucide-react'
import type { OrderBookResponse } from '../types'

interface Props {
  markets: OrderBookResponse[]
  selectedTokenId: string | null
  onSelect: (tokenId: string) => void
}

function MarketList({ markets, selectedTokenId, onSelect }: Props) {
  const [query, setQuery] = useState('')

  const filtered = useMemo(() => {
    if (!query) return markets
    const q = query.toLowerCase()
    return markets.filter((m) =>
      m.token_id.toLowerCase().includes(q) ||
      (m.market_title ?? '').toLowerCase().includes(q)
    )
  }, [markets, query])

  return (
    <div className="card flex flex-col h-full">
      {/* Search */}
      <div className="relative mb-3">
        <Search className="absolute left-2.5 top-1/2 -translate-y-1/2 w-3.5 h-3.5 text-slate-500" />
        <input
          type="text"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Search markets…"
          className="w-full bg-slate-800 border border-slate-700 rounded px-8 py-1.5 text-slate-200 text-[11px] placeholder:text-slate-600 focus:outline-none focus:border-slate-500"
        />
      </div>

      {/* List */}
      <div className="flex-1 overflow-y-auto -mx-1">
        {filtered.length === 0 ? (
          <p className="text-slate-500 text-[11px] text-center py-8">
            No markets subscribed
          </p>
        ) : (
          filtered.map((m) => {
            const selected = m.token_id === selectedTokenId
            const mid =
              m.best_bid != null && m.best_ask != null
                ? (m.best_bid + m.best_ask) / 2
                : null

            return (
              <button
                key={m.token_id}
                onClick={() => onSelect(m.token_id)}
                className={`w-full text-left px-3 py-2 text-[11px] flex items-center gap-2 transition-colors ${
                  selected
                    ? 'bg-slate-800 border-l-2 border-emerald-400'
                    : 'hover:bg-slate-800/50 border-l-2 border-transparent'
                }`}
              >
                <span
                  className={`shrink-0 ${
                    selected ? 'text-emerald-400' : 'text-slate-600'
                  }`}
                >
                  ●
                </span>

                <span className="text-slate-200 truncate min-w-0 flex-1">
                  {m.market_title
                    ? m.market_title
                    : m.token_id.length > 12
                      ? `${m.token_id.slice(0, 12)}…`
                      : m.token_id}
                </span>

                {mid != null && (
                  <span className="text-slate-400 tabular-nums shrink-0">
                    ${mid.toFixed(3)}
                  </span>
                )}

                {m.spread_bps != null && (
                  <span className="text-[10px] px-1.5 py-0.5 rounded bg-slate-700 text-slate-400 tabular-nums shrink-0">
                    {m.spread_bps}bp
                  </span>
                )}
              </button>
            )
          })
        )}
      </div>
    </div>
  )
}

export default memo(MarketList)
