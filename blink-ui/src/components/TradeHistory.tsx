import { useState } from 'react'
import { ChevronLeft, ChevronRight } from 'lucide-react'
import { usePoll } from '../hooks/usePoll'
import { api } from '../lib/api'
import { fmt, fmtPnl, pnlClass, fmtNeonTime, formatEventTiming } from '../lib/format'
import MarketLink from './MarketLink'

function fmtDuration(secs: number): string {
  if (secs < 60) return `${Math.round(secs)}s`
  if (secs < 3600) return `${Math.round(secs / 60)}m`
  return `${(secs / 3600).toFixed(1)}h`
}

export default function TradeHistory() {
  const [page, setPage] = useState(1)
  const { data, loading } = usePoll(
    () => api.history(page, 20),
    5_000,
  )

  const trades = data?.trades ?? []
  const total = data?.total ?? 0
  const totalPages = Math.max(1, Math.ceil(total / 20))

  return (
    <div className="card">
      <div className="flex items-center justify-between mb-3">
        <span className="text-xs font-semibold uppercase tracking-widest text-slate-500">
          Trade History
        </span>
        <span className="badge badge-neutral">{total} total</span>
      </div>

      {loading && trades.length === 0 ? (
        <p className="text-slate-600 text-xs text-center py-6">Loading…</p>
      ) : trades.length === 0 ? (
        <p className="text-slate-600 text-xs text-center py-6">No closed trades</p>
      ) : (
        <>
          <div className="overflow-x-auto">
            <table className="w-full text-xs">
              <thead>
                <tr className="text-slate-500 border-b border-surface-600">
                  <th className="text-left pb-2 pr-3 font-normal">Market</th>
                  <th className="text-left pb-2 pr-3 font-normal">Closed</th>
                  <th className="text-right pb-2 pr-3 font-normal">Size</th>
                  <th className="text-right pb-2 pr-3 font-normal">Duration</th>
                  <th className="text-right pb-2 pr-3 font-normal">Event</th>
                  <th className="text-left pb-2 pr-3 font-normal">Reason</th>
                  <th className="text-right pb-2 font-normal">P&amp;L</th>
                </tr>
              </thead>
              <tbody>
                {trades.map((t, i) => {
                  const event = formatEventTiming(t.event_start_time, t.event_end_time)
                  return (
                  <tr key={i} className="border-b border-surface-700/50 hover:bg-surface-700/30">
                    <td className="py-1.5 pr-3 font-mono text-slate-200 max-w-[120px] truncate">
                      <MarketLink
                        tokenId={t.token_id}
                        label={t.market_title ?? t.token_id}
                        titleOverride={t.market_title ?? t.token_id}
                      />
                    </td>
                    <td className="py-1.5 pr-3 font-mono text-cyan-400">
                      {t.closed_at ? fmtNeonTime(t.closed_at) : '—'}
                    </td>
                    <td className="py-1.5 pr-3 font-mono text-slate-300 text-right">
                      {t.shares} @ ${fmt(t.entry_price, 4)}
                    </td>
                    <td className="py-1.5 pr-3 font-mono text-slate-400 text-right">
                      {t.duration_secs > 0 ? fmtDuration(t.duration_secs) : '—'}
                    </td>
                    <td className={`py-1.5 pr-3 font-mono text-right text-[10px] ${event.className}`}>
                      {event.text}
                    </td>
                    <td className="py-1.5 pr-3 text-slate-500 max-w-[80px] truncate" title={t.reason}>
                      {t.reason || '—'}
                    </td>
                    <td className={`py-1.5 font-mono font-semibold text-right ${pnlClass(t.realized_pnl)}`}>
                      {fmtPnl(t.realized_pnl)}
                    </td>
                  </tr>
                  )
                })}
              </tbody>
            </table>
          </div>

          <div className="flex items-center justify-between mt-3 text-xs text-slate-500">
            <button
              onClick={() => setPage((p) => Math.max(1, p - 1))}
              disabled={page === 1}
              className="flex items-center gap-1 hover:text-slate-300 disabled:opacity-30"
            >
              <ChevronLeft size={13} /> Prev
            </button>
            <span>Page {page} / {totalPages}</span>
            <button
              onClick={() => setPage((p) => Math.min(totalPages, p + 1))}
              disabled={page === totalPages}
              className="flex items-center gap-1 hover:text-slate-300 disabled:opacity-30"
            >
              Next <ChevronRight size={13} />
            </button>
          </div>
        </>
      )}
    </div>
  )
}
