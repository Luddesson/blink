import { useState, useMemo } from 'react'
import { ChevronLeft, ChevronRight, Search } from 'lucide-react'
import type { ClosedTrade } from '../../types'
import { fmt, fmtPnl, pnlClass, fmtNeonTime, fmtDuration, formatEventTiming } from '../../lib/format'
import MarketLink from '../MarketLink'

interface Props {
  trades: ClosedTrade[]
}

type SortField = 'closed_at' | 'realized_pnl' | 'duration_secs' | 'slippage_bps' | 'fees_paid_usdc' | 'shares'
type SortDir = 'asc' | 'desc'

const PAGE_SIZES = [20, 50, 100]

export default function EnhancedTradeTable({ trades }: Props) {
  const [page, setPage] = useState(1)
  const [pageSize, setPageSize] = useState(20)
  const [search, setSearch] = useState('')
  const [sortField, setSortField] = useState<SortField>('closed_at')
  const [sortDir, setSortDir] = useState<SortDir>('desc')

  const filtered = useMemo(() => {
    if (!search) return trades
    const q = search.toLowerCase()
    return trades.filter(t =>
      (t.market_title ?? t.token_id).toLowerCase().includes(q) ||
      t.reason.toLowerCase().includes(q) ||
      (t.signal_source ?? '').toLowerCase().includes(q)
    )
  }, [trades, search])

  const sorted = useMemo(() => {
    return [...filtered].sort((a, b) => {
      let cmp = 0
      switch (sortField) {
        case 'closed_at': cmp = new Date(a.closed_at).getTime() - new Date(b.closed_at).getTime(); break
        case 'realized_pnl': cmp = a.realized_pnl - b.realized_pnl; break
        case 'duration_secs': cmp = a.duration_secs - b.duration_secs; break
        case 'slippage_bps': cmp = a.slippage_bps - b.slippage_bps; break
        case 'fees_paid_usdc': cmp = a.fees_paid_usdc - b.fees_paid_usdc; break
        case 'shares': cmp = a.shares - b.shares; break
      }
      return sortDir === 'asc' ? cmp : -cmp
    })
  }, [filtered, sortField, sortDir])

  const totalPages = Math.max(1, Math.ceil(sorted.length / pageSize))
  const paginated = sorted.slice((page - 1) * pageSize, page * pageSize)

  function toggleSort(field: SortField) {
    if (sortField === field) setSortDir(d => d === 'asc' ? 'desc' : 'asc')
    else { setSortField(field); setSortDir('desc') }
    setPage(1)
  }

  function SortTh({ field, label, align = 'left' }: { field: SortField; label: string; align?: string }) {
    return (
      <th
        className={`pb-2 pr-3 font-normal cursor-pointer hover:text-slate-300 select-none ${align === 'right' ? 'text-right' : 'text-left'}`}
        onClick={() => toggleSort(field)}
      >
        {label}
        {sortField === field && <span className="ml-1">{sortDir === 'asc' ? '▲' : '▼'}</span>}
      </th>
    )
  }

  return (
    <div className="card">
      <div className="flex items-center justify-between mb-3">
        <span className="text-xs font-semibold uppercase tracking-widest text-slate-500">
          Trade Log
        </span>
        <div className="flex items-center gap-3">
          <div className="relative">
            <Search size={12} className="absolute left-2 top-1/2 -translate-y-1/2 text-slate-500" />
            <input
              type="text"
              value={search}
              onChange={e => { setSearch(e.target.value); setPage(1) }}
              placeholder="Filter markets..."
              className="bg-surface-700 text-xs text-slate-300 rounded pl-7 pr-2 py-1 w-[160px]
                border border-surface-600 focus:border-cyan-500/50 focus:outline-none"
            />
          </div>
          <select
            value={pageSize}
            onChange={e => { setPageSize(Number(e.target.value)); setPage(1) }}
            className="bg-surface-700 text-xs text-slate-400 rounded px-1.5 py-1 border border-surface-600"
          >
            {PAGE_SIZES.map(s => <option key={s} value={s}>{s}/page</option>)}
          </select>
          <span className="badge badge-neutral text-[10px]">{sorted.length} trades</span>
        </div>
      </div>

      {sorted.length === 0 ? (
        <p className="text-slate-600 text-xs text-center py-6">No trades match filter</p>
      ) : (
        <>
          <div className="overflow-x-auto">
            <table className="w-full text-xs">
              <thead>
                <tr className="text-slate-500 border-b border-surface-600">
                  <th className="text-left pb-2 pr-3 font-normal">Market</th>
                  <SortTh field="closed_at" label="Closed" />
                  <SortTh field="shares" label="Size" align="right" />
                  <SortTh field="duration_secs" label="Duration" align="right" />
                  <th className="text-right pb-2 pr-3 font-normal">Event</th>
                  <th className="text-left pb-2 pr-3 font-normal">Reason</th>
                  <th className="text-left pb-2 pr-3 font-normal">Source</th>
                  <SortTh field="slippage_bps" label="Slip" align="right" />
                  <SortTh field="fees_paid_usdc" label="Fees" align="right" />
                  <SortTh field="realized_pnl" label="P&L" align="right" />
                </tr>
              </thead>
              <tbody>
                {paginated.map((t, i) => {
                  const event = formatEventTiming(t.event_start_time, t.event_end_time)
                  return (
                    <tr key={i} className="border-b border-surface-700/50 hover:bg-surface-700/30">
                      <td className="py-1.5 pr-3 font-mono text-slate-200 max-w-[140px] truncate">
                        <MarketLink tokenId={t.token_id} label={t.market_title ?? t.token_id} titleOverride={t.market_title ?? t.token_id} />
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
                      <td className="py-1.5 pr-3">
                        {t.signal_source && (
                          <span className={`text-[9px] font-semibold uppercase px-1.5 py-0.5 rounded-full ${
                            t.signal_source === 'alpha'
                              ? 'bg-blue-900/40 text-blue-300'
                              : 'bg-amber-900/40 text-amber-300'
                          }`}>
                            {t.signal_source}
                          </span>
                        )}
                      </td>
                      <td className="py-1.5 pr-3 font-mono text-slate-500 text-right">
                        {fmt(t.slippage_bps, 1)}
                      </td>
                      <td className="py-1.5 pr-3 font-mono text-slate-500 text-right">
                        ${fmt(t.fees_paid_usdc, 4)}
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
              onClick={() => setPage(p => Math.max(1, p - 1))}
              disabled={page === 1}
              className="flex items-center gap-1 hover:text-slate-300 disabled:opacity-30"
            >
              <ChevronLeft size={13} /> Prev
            </button>
            <span>Page {page} / {totalPages}</span>
            <button
              onClick={() => setPage(p => Math.min(totalPages, p + 1))}
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
