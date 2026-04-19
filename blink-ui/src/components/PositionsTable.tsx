import React, { useState, useMemo, useCallback, memo } from 'react'
import { ChevronDown, ChevronUp, X } from 'lucide-react'
import type { Position } from '../types'
import { fmt, fmtPnl, fmtDuration, pnlClass, formatEventTiming, fmtNeonTime } from '../lib/format'
import { api } from '../lib/api'
import MarketLink from './MarketLink'

interface Props {
  positions: Position[]
  loading?: boolean
  isLive?: boolean
  onRefresh?: () => void
}

type SortKey = 'market_title' | 'shares' | 'entry_price' | 'current_price' | 'unrealized_pnl'

const FRACTIONS = [
  { label: '25%', value: 0.25 },
  { label: '50%', value: 0.5 },
  { label: '75%', value: 0.75 },
  { label: 'All', value: 1.0 },
]

function PositionsTable({ positions, loading, onRefresh }: Props) {
  const [sort, setSort] = useState<SortKey>('current_price')
  const [asc, setAsc] = useState(false)
  // Which position has the close panel open
  const [closingId, setClosingId] = useState<number | null>(null)
  const [pendingFraction, setPendingFraction] = useState<number>(1.0)
  // In-flight sell request
  const [selling, setSelling] = useState<number | null>(null)
  // Optimistically hidden positions (sold successfully) — keyed by id, value is sell timestamp.
  // Auto-cleared after 10s to allow WS to catch up.
  const [soldIds, setSoldIds] = useState<Map<number, number>>(new Map())
  // Per-position error messages
  const [errors, setErrors] = useState<Record<number, string>>({})
  // Per-position P&L flash after close (disappears after 4s)
  const [pnlFlash, setPnlFlash] = useState<Record<number, { pnl: number; label: string }>>({})
  // Close-all in flight
  const [closingAll, setClosingAll] = useState(false)

  function toggleSort(key: SortKey) {
    if (sort === key) setAsc(!asc)
    else { setSort(key); setAsc(false) }
  }

  const visible = useMemo(() => {
    const now = Date.now()
    // Keep hiding for 10s after sell to let WS snapshot catch up
    const activeHides = new Set(
      [...soldIds.entries()]
        .filter(([, ts]) => now - ts < 10_000)
        .map(([id]) => id)
    )
    return positions.filter((p) => !activeHides.has(p.id))
  }, [positions, soldIds])

  const sorted = useMemo(() => [...visible].sort((a, b) => {
    const va = a[sort] as number | string | undefined
    const vb = b[sort] as number | string | undefined
    const sa = va ?? ''
    const sb = vb ?? ''
    if (typeof sa === 'string') return asc ? sa.localeCompare(sb as string) : (sb as string).localeCompare(sa)
    return asc ? (sa as number) - (sb as number) : (sb as number) - (sa as number)
  }), [visible, sort, asc])

  const flashPnl = useCallback((id: number, pnl: number, fraction: number) => {
    const label = fraction < 1 ? `${Math.round(fraction * 100)}%` : 'closed'
    setPnlFlash((prev) => ({ ...prev, [id]: { pnl, label } }))
    setTimeout(() => setPnlFlash((prev) => { const n = { ...prev }; delete n[id]; return n }), 4000)
  }, [])

  async function handleSell(id: number, fraction: number) {
    setSelling(id)
    setErrors((prev) => { const n = { ...prev }; delete n[id]; return n })
    // Retry up to 2 times on transient failures
    for (let attempt = 0; attempt < 3; attempt++) {
      try {
        const res = await api.sellPosition(id, fraction)
        if (fraction >= 1.0) {
          setSoldIds((prev) => new Map([...prev, [id, Date.now()]]))
          setClosingId(null)
        } else {
          setClosingId(null)
          onRefresh?.()
        }
        flashPnl(id, res.realized_pnl, fraction)
        setSelling(null)
        return
      } catch (e) {
        if (attempt === 2) {
          setErrors((prev) => ({ ...prev, [id]: String(e) }))
        }
        // Brief pause before retry
        await new Promise(r => setTimeout(r, 500))
      }
    }
    setSelling(null)
  }

  async function handleCloseAll() {
    if (visible.length === 0) return
    setClosingAll(true)
    try {
      await Promise.all(visible.map((p) => api.sellPosition(p.id, 1.0).catch(() => null)))
      setSoldIds(new Map(visible.map((p) => [p.id, Date.now()])))
      onRefresh?.()
    } finally {
      setClosingAll(false)
    }
  }

  function SortIcon({ k }: { k: SortKey }) {
    if (sort !== k) return <span className="opacity-20">↕</span>
    return asc ? <ChevronUp size={12} className="inline" /> : <ChevronDown size={12} className="inline" />
  }

  function CloseFractionControls({ id, isSelling, errMsg }: { id: number; isSelling: boolean; errMsg?: string }) {
    return (
      <div className="flex flex-wrap items-center gap-2">
        <span className="text-[10px] font-semibold uppercase tracking-wider text-slate-400">
          Close fraction:
        </span>
        {FRACTIONS.map((f) => (
          <button
            key={f.value}
            onClick={() => setPendingFraction(f.value)}
            className={`px-3 py-1 rounded text-[11px] font-mono font-semibold border transition-colors ${
              pendingFraction === f.value
                ? 'bg-rose-700 text-white border-rose-500'
                : 'bg-surface-800 text-slate-400 border-slate-700 hover:border-slate-500 hover:text-slate-200'
            }`}
          >
            {f.label}
          </button>
        ))}
        <button
          onClick={() => handleSell(id, pendingFraction)}
          disabled={isSelling}
          className="px-4 py-1 rounded text-[11px] font-semibold bg-rose-600 hover:bg-rose-500 text-white border border-rose-400/50 disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
        >
          {isSelling
            ? '⏳ Closing…'
            : `Confirm close ${pendingFraction < 1 ? Math.round(pendingFraction * 100) + '%' : 'all'}`}
        </button>
        <button
          onClick={() => setClosingId(null)}
          className="text-[10px] text-slate-600 hover:text-slate-400"
        >
          Cancel
        </button>
        {errMsg && (
          <span className="text-[10px] text-rose-400 font-mono">
            ✗ {errMsg}
          </span>
        )}
      </div>
    )
  }

  return (
    <div className="card">
      <div className="flex items-center justify-between mb-3">
        <span className="text-xs font-semibold uppercase tracking-widest text-slate-500">
          Open Positions
        </span>
        <div className="flex items-center gap-2">
          {visible.length > 1 && (
            <button
              onClick={handleCloseAll}
              disabled={closingAll}
              className="px-2 py-0.5 rounded text-[10px] font-semibold bg-rose-900/60 text-rose-300 hover:bg-rose-800/80 border border-rose-700/50 disabled:opacity-40 transition-colors"
            >
              {closingAll ? 'Closing…' : 'Close All'}
            </button>
          )}
          <span className="badge badge-neutral">{visible.length}</span>
        </div>
      </div>

      {loading ? (
        <div className="space-y-2 py-2">
          {[...Array(3)].map((_, i) => (
            <div key={i} className="h-8 bg-surface-700 rounded animate-pulse" />
          ))}
        </div>
      ) : visible.length === 0 ? (
        <p className="text-slate-600 text-xs text-center py-6">No open positions</p>
      ) : (
        <>
          <div className="space-y-3 md:hidden">
            {sorted.map((p) => {
            const cost = p.usdc_spent
            const currentValue = p.shares * p.current_price
            const maxProfit = p.side === 'Buy'
              ? (1 - p.entry_price) * p.shares
              : p.entry_price * p.shares
            const toWin = maxProfit - p.unrealized_pnl
            const nearAutoClose = toWin > 0 && toWin / maxProfit < 0.2
            const isExpanded = closingId === p.id
            const isSelling = selling === p.id
            const flash = pnlFlash[p.id]
            const errMsg = errors[p.id]
            let closesText: string
            let closesClass: string
            const secsLeft = p.secs_to_event
            if (secsLeft !== undefined) {
              if (secsLeft < 0) { closesText = 'ENDED'; closesClass = 'text-slate-500' }
              else if (secsLeft < 60) { closesText = `⚠ ${secsLeft}s`; closesClass = 'text-red-400 font-bold' }
              else if (secsLeft < 600) { closesText = fmtDuration(secsLeft); closesClass = 'text-orange-400' }
              else if (secsLeft < 3600) { closesText = fmtDuration(secsLeft); closesClass = 'text-amber-400' }
              else { closesText = fmtDuration(secsLeft); closesClass = 'text-slate-400' }
            } else {
              const ev = formatEventTiming(p.event_start_time, p.event_end_time)
              closesText = ev.text
              closesClass = ev.className
            }

            return (
              <div key={p.id} className="rounded-xl border border-surface-700/60 bg-surface-800/40 p-3">
                <div className="mb-2 flex items-start justify-between gap-3">
                  <div className="min-w-0">
                    <div className={`inline-flex max-w-full items-center gap-1 rounded px-2 py-0.5 text-[10px] font-mono font-semibold ${
                      p.side === 'Buy'
                        ? 'bg-emerald-900/60 text-emerald-300 border border-emerald-700/50'
                        : 'bg-pink-900/60 text-pink-300 border border-pink-700/50'
                    }`}>
                      {p.market_outcome ?? (p.side === 'Buy' ? 'YES' : 'NO')}
                    </div>
                    <div className="mt-2 truncate font-mono text-sm text-slate-100">
                      <MarketLink
                        tokenId={p.token_id}
                        label={p.market_title ?? p.token_id}
                        titleOverride={p.market_title ?? p.token_id}
                      />
                    </div>
                  </div>
                  <div className="text-right">
                    <div className={`font-mono text-sm font-semibold ${pnlClass(p.unrealized_pnl)}`}>
                      {fmtPnl(p.unrealized_pnl)}
                    </div>
                    <div className="text-[10px] text-slate-500">
                      {p.opened_at && <span className="text-cyan-400">{fmtNeonTime(p.opened_at)} </span>}
                      {fmtDuration(p.opened_age_secs)}
                    </div>
                  </div>
                </div>

                <div className="grid grid-cols-2 gap-x-3 gap-y-2 text-[11px]">
                  <div>
                    <div className="text-slate-500">Entry</div>
                    <div className="font-mono text-slate-200">{fmt(p.entry_price, 4)}</div>
                  </div>
                  <div>
                    <div className="text-slate-500">Price</div>
                    <div className="font-mono text-slate-200">{fmt(p.current_price, 4)}</div>
                  </div>
                  <div>
                    <div className="text-slate-500">Shares</div>
                    <div className="font-mono text-slate-300">{p.shares}</div>
                  </div>
                  <div>
                    <div className="text-slate-500">Stake</div>
                    <div className="font-mono text-slate-300">${fmt(cost)}</div>
                  </div>
                  <div>
                    <div className="text-slate-500">Value</div>
                    <div className={`font-mono ${currentValue >= cost ? 'text-emerald-400' : 'text-rose-400'}`}>${fmt(currentValue)}</div>
                  </div>
                  <div>
                    <div className="text-slate-500">Closes</div>
                    <div className={`font-mono ${closesClass}`}>{closesText}</div>
                  </div>
                  <div className="col-span-2">
                    <div className="text-slate-500">To win</div>
                    <div className={`font-mono ${nearAutoClose ? 'text-emerald-300 font-bold' : 'text-slate-400'}`}>
                      {nearAutoClose ? 'AUTO NOW' : `$${fmt(Math.max(0, toWin))}`}
                    </div>
                  </div>
                </div>

                <div className="mt-3 border-t border-surface-700/60 pt-3">
                  {flash ? (
                    <span className={`font-mono text-[11px] font-semibold ${flash.pnl >= 0 ? 'text-emerald-400' : 'text-rose-400'}`}>
                      {flash.pnl >= 0 ? '+' : ''}{fmtPnl(flash.pnl)} ✓
                    </span>
                  ) : isExpanded ? (
                    <CloseFractionControls id={p.id} isSelling={isSelling} errMsg={errMsg} />
                  ) : (
                    <button
                      onClick={() => { setClosingId(p.id); setPendingFraction(1.0) }}
                      disabled={isSelling}
                      className="w-full rounded border border-rose-700/40 bg-rose-900/50 px-3 py-2 text-[11px] font-semibold text-rose-300 transition-colors hover:bg-rose-800 disabled:opacity-40"
                    >
                      Close
                    </button>
                  )}
                </div>
              </div>
            )
            })}
          </div>
          <div className="hidden overflow-x-auto md:block">
            <table className="w-full text-xs">
            <thead>
              <tr className="text-slate-500 border-b border-surface-600">
                <th className="text-left pb-2 pr-3 font-normal w-10">Side</th>
                {([
                  ['market_title', 'Market'],
                  ['shares', 'Shares'],
                  ['entry_price', 'Entry'],
                  ['current_price', 'Price'],
                  ['unrealized_pnl', 'uPnL'],
                ] as [SortKey, string][]).map(([k, label]) => (
                  <th
                    key={k}
                    className="text-left pb-2 pr-3 font-normal cursor-pointer hover:text-slate-300 select-none"
                    onClick={() => toggleSort(k)}
                  >
                    {label} <SortIcon k={k} />
                  </th>
                ))}
                <th className="text-right pb-2 pr-3 font-normal">Stake</th>
                <th className="text-right pb-2 pr-3 font-normal">Value</th>
                <th className="text-right pb-2 pr-3 font-normal">To Win</th>
                <th className="text-right pb-2 pr-3 font-normal">Closes</th>
                <th className="text-right pb-2 pr-3 font-normal">Age</th>
                <th className="pb-2 text-right font-normal">Action</th>
              </tr>
            </thead>
            <tbody>
              {sorted.map((p) => {
                const cost = p.usdc_spent
                const currentValue = p.shares * p.current_price
                const maxProfit = p.side === 'Buy'
                  ? (1 - p.entry_price) * p.shares
                  : p.entry_price * p.shares
                const toWin = maxProfit - p.unrealized_pnl
                const nearAutoClose = toWin > 0 && toWin / maxProfit < 0.2
                let closesText: string
                let closesClass: string
                const secsLeft = p.secs_to_event
                if (secsLeft !== undefined) {
                  if (secsLeft < 0) { closesText = 'ENDED'; closesClass = 'text-slate-500' }
                  else if (secsLeft < 60) { closesText = `⚠ ${secsLeft}s`; closesClass = 'text-red-400 font-bold' }
                  else if (secsLeft < 600) { closesText = fmtDuration(secsLeft); closesClass = 'text-orange-400' }
                  else if (secsLeft < 3600) { closesText = fmtDuration(secsLeft); closesClass = 'text-amber-400' }
                  else { closesText = fmtDuration(secsLeft); closesClass = 'text-slate-400' }
                } else {
                  const ev = formatEventTiming(p.event_start_time, p.event_end_time)
                  closesText = ev.text; closesClass = ev.className
                }

                const isExpanded = closingId === p.id
                const isSelling = selling === p.id
                const flash = pnlFlash[p.id]
                const errMsg = errors[p.id]

                return (
                  <React.Fragment key={p.id}>
                    <tr className={`border-b border-surface-700/50 transition-colors ${isExpanded ? 'bg-surface-700/40' : 'hover:bg-surface-700/30'}`}>
                      <td className="py-2 pr-3 text-left">
                        <span className={`inline-block px-2 py-0.5 rounded text-[10px] font-mono font-semibold truncate max-w-[100px] ${
                          p.side === 'Buy'
                            ? 'bg-emerald-900/60 text-emerald-300 border border-emerald-700/50'
                            : 'bg-pink-900/60 text-pink-300 border border-pink-700/50'
                        }`} title={p.market_outcome ?? p.side}>
                          {p.market_outcome
                            ? <>{p.market_outcome} <span className="opacity-50">{p.side === 'Buy' ? '▲' : '▼'}</span></>
                            : (p.side === 'Buy' ? '▲ YES' : '▼ NO')}
                        </span>
                      </td>
                      <td className="py-2 pr-3 font-mono text-slate-200 max-w-[160px] truncate">
                        <MarketLink
                          tokenId={p.token_id}
                          label={p.market_title ?? p.token_id}
                          titleOverride={p.market_title ?? p.token_id}
                        />
                      </td>
                      <td className="py-2 pr-3 font-mono text-slate-300">{p.shares}</td>
                      <td className="py-2 pr-3 font-mono text-slate-300">{fmt(p.entry_price, 4)}</td>
                      <td className="py-2 pr-3 font-mono text-slate-200">{fmt(p.current_price, 4)}</td>
                      <td className={`py-2 pr-3 font-mono font-semibold ${pnlClass(p.unrealized_pnl)}`}>
                        {fmtPnl(p.unrealized_pnl)}
                      </td>
                      <td className="py-2 pr-3 font-mono text-right text-slate-400 text-[10px]">
                        ${fmt(cost)}
                      </td>
                      <td className={`py-2 pr-3 font-mono text-right text-[10px] ${currentValue >= cost ? 'text-emerald-400' : 'text-rose-400'}`}>
                        ${fmt(currentValue)}
                      </td>
                      <td className={`py-2 pr-3 font-mono text-right text-[10px] ${nearAutoClose ? 'text-emerald-300 font-bold' : 'text-slate-400'}`}>
                        {nearAutoClose ? 'AUTO NOW' : `$${fmt(Math.max(0, toWin))}`}
                      </td>
                      <td className={`py-2 pr-3 font-mono text-right text-[10px] ${closesClass}`}>
                        {closesText}
                      </td>
                      <td className="py-2 pr-3 font-mono text-right text-slate-500 text-[10px]">
                        {p.opened_at && <span className="text-cyan-400">{fmtNeonTime(p.opened_at)} </span>}
                        {fmtDuration(p.opened_age_secs)}
                      </td>
                      <td className="py-2 text-right">
                        {flash ? (
                          <span className={`font-mono text-[10px] font-semibold ${flash.pnl >= 0 ? 'text-emerald-400' : 'text-rose-400'}`}>
                            {flash.pnl >= 0 ? '+' : ''}{fmtPnl(flash.pnl)} ✓
                          </span>
                        ) : isExpanded ? (
                          <button
                            onClick={() => setClosingId(null)}
                            className="p-1 rounded text-slate-500 hover:text-slate-300"
                            title="Cancel"
                          >
                            <X size={12} />
                          </button>
                        ) : (
                          <button
                            onClick={() => { setClosingId(p.id); setPendingFraction(1.0) }}
                            disabled={isSelling}
                            className="px-2 py-1 rounded text-[10px] font-semibold bg-rose-900/50 text-rose-300 hover:bg-rose-800 border border-rose-700/40 disabled:opacity-40 transition-colors"
                          >
                            Close
                          </button>
                        )}
                      </td>
                    </tr>

                    {/* ── Inline close panel ──────────────────────────────── */}
                    {isExpanded && (
                      <tr className="border-b border-rose-900/30 bg-rose-950/20">
                        <td colSpan={12} className="px-3 py-2">
                          <CloseFractionControls id={p.id} isSelling={isSelling} errMsg={errMsg} />
                        </td>
                      </tr>
                    )}
                  </React.Fragment>
                )
              })}
            </tbody>
            </table>
          </div>
        </>
      )}
    </div>
  )
}

export default memo(PositionsTable)
