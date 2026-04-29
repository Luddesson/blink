import { useState, useMemo, memo } from 'react'
import { motion, AnimatePresence } from 'motion/react'
import { ChevronDown, ChevronUp, Clock, TrendingUp, TrendingDown } from 'lucide-react'
import type { Position } from '../types'
import { fmtDuration, pnlClass } from '../lib/format'
import MarketLink from './MarketLink'
import NumberFlip from './motion/NumberFlip'
import { cn } from '../lib/cn'

interface Props {
  positions: Position[]
  loading?: boolean
  isLive?: boolean
  onRefresh?: () => void
}

type SortKey = 'market_title' | 'shares' | 'entry_price' | 'current_price' | 'unrealized_pnl'

function PositionsTable({ positions, isLive }: Props) {
  const [sort, setSort] = useState<SortKey>('current_price')
  const [asc, setAsc] = useState(false)
  const [closingId, setClosingId] = useState<number | null>(null)

  function toggleSort(key: SortKey) {
    if (sort === key) setAsc(!asc)
    else { setSort(key); setAsc(false) }
  }

  const visible = useMemo(() => {
    return positions
  }, [positions])
  const title = isLive ? 'Blink Positions' : 'Open Positions'
  const emptyLabel = isLive ? 'No Blink positions' : 'No open positions'

  const sorted = useMemo(() => [...visible].sort((a, b) => {
    const va = a[sort] as number | string | undefined
    const vb = b[sort] as number | string | undefined
    const sa = va ?? ''
    const sb = vb ?? ''
    if (typeof sa === 'string') return asc ? sa.localeCompare(sb as string) : (sb as string).localeCompare(sa)
    return asc ? (sa as number) - (sb as number) : (sb as number) - (sa as number)
  }), [visible, sort, asc])

  function SortIcon({ k }: { k: SortKey }) {
    if (sort !== k) return <span className="opacity-20 text-[8px]">▼</span>
    return asc ? <ChevronUp size={12} className="inline text-[color:var(--color-aurora-1)]" /> : <ChevronDown size={12} className="inline text-[color:var(--color-aurora-1)]" />
  }

  return (
    <div className="card-compact overflow-hidden border border-[color:var(--color-border-subtle)] bg-[color:var(--color-surface-900)/0.5] backdrop-blur-xl">
      <div className="flex items-center justify-between px-2 mb-4 mt-2">
        <div className="flex items-center gap-2.5">
          <div className="p-1.5 rounded-md bg-[color:var(--color-aurora-1)/0.1]">
            <TrendingUp size={14} className="text-[color:var(--color-aurora-1)]" />
          </div>
          <span className="text-[11px] font-black uppercase tracking-[0.2em] text-[color:var(--color-text-secondary)]">
            {title}
          </span>
          <span className="px-1.5 py-0.5 rounded-full bg-[color:var(--color-surface-700)] text-[10px] font-bold tabular font-mono">{visible.length}</span>
        </div>
      </div>

      <div className="overflow-x-auto">
        <table className="w-full text-left border-collapse">
          <thead>
            <tr className="border-b border-[color:var(--color-border-subtle)] bg-[color:var(--color-surface-950)/0.3]">
              <th className="py-2.5 px-3 text-[10px] uppercase tracking-widest text-[color:var(--color-text-dim)] font-bold">Side</th>
              {([
                ['market_title', 'Market'],
                ['shares', 'Size'],
                ['entry_price', 'Entry'],
                ['current_price', 'Mark'],
                ['unrealized_pnl', 'uPnL'],
              ] as [SortKey, string][]).map(([k, label]) => (
                <th
                  key={k}
                  className="py-2.5 px-3 text-[10px] uppercase tracking-widest text-[color:var(--color-text-dim)] font-bold cursor-pointer hover:text-[color:var(--color-text-primary)] transition-colors select-none"
                  onClick={() => toggleSort(k)}
                >
                  <div className="flex items-center gap-1.5">
                    {label} <SortIcon k={k} />
                  </div>
                </th>
              ))}
              <th className="py-2.5 px-3 text-[10px] uppercase tracking-widest text-[color:var(--color-text-dim)] font-bold text-right">Age</th>
              <th className="py-2.5 px-3 text-[10px] uppercase tracking-widest text-[color:var(--color-text-dim)] font-bold text-right">Action</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-[color:var(--color-border-subtle)/0.5]">
            <AnimatePresence mode="popLayout">
              {sorted.map((p) => {
                const isExpanded = closingId === p.id

                return (
                  <motion.tr
                    key={p.id}
                    layout
                    initial={{ opacity: 0, x: -10 }}
                    animate={{ opacity: 1, x: 0 }}
                    exit={{ opacity: 0, scale: 0.95 }}
                    transition={{ type: 'spring', stiffness: 350, damping: 25 }}
                    className={cn(
                      "group transition-colors",
                      isExpanded ? "bg-[color:var(--color-surface-800)/0.4]" : "hover:bg-[color:var(--color-surface-800)/0.2]"
                    )}
                  >
                    <td className="py-3 px-3">
                      <span className={cn(
                        "inline-flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px] font-black uppercase tracking-wider border",
                        p.side === 'Buy'
                          ? "bg-[color:var(--color-bull-500)/0.1] text-[color:var(--color-bull-400)] border-[color:var(--color-bull-500)/0.3]"
                          : "bg-[color:var(--color-bear-500)/0.1] text-[color:var(--color-bear-400)] border-[color:var(--color-bear-500)/0.3]"
                      )}>
                        {p.side === 'Buy' ? 'Long' : 'Short'}
                      </span>
                    </td>
                    <td className="py-3 px-3 min-w-[200px] max-w-[300px]">
                      <div className="text-xs font-medium text-[color:var(--color-text-secondary)] truncate group-hover:text-[color:var(--color-text-primary)] transition-colors">
                        <MarketLink
                          tokenId={p.token_id}
                          label={p.market_title ?? p.token_id}
                          titleOverride={p.market_title ?? p.token_id}
                        />
                      </div>
                      {p.market_outcome && (
                        <div className="text-[9px] text-[color:var(--color-text-dim)] uppercase tracking-tighter mt-0.5">{p.market_outcome}</div>
                      )}
                    </td>
                    <td className="py-3 px-3 tabular font-mono text-[11px] text-[color:var(--color-text-secondary)]">
                      {p.shares.toLocaleString()}
                    </td>
                    <td className="py-3 px-3 tabular font-mono text-[11px] text-[color:var(--color-text-dim)]">
                      {p.entry_price.toFixed(3)}
                    </td>
                    <td className="py-3 px-3">
                      <NumberFlip
                        value={p.current_price}
                        format={(v) => v.toFixed(3)}
                        className="text-[11px] font-bold"
                      />
                    </td>
                    <td className="py-3 px-3">
                      <div className={cn("text-[11px] font-black tabular font-mono", pnlClass(p.unrealized_pnl))}>
                        <NumberFlip
                          value={p.unrealized_pnl}
                          format={(v) => (v >= 0 ? '+' : '') + v.toFixed(2)}
                        />
                        <span className="ml-1 opacity-60 text-[9px]">{p.unrealized_pnl_pct.toFixed(1)}%</span>
                      </div>
                    </td>
                    <td className="py-3 px-3 text-right">
                      <div className="flex flex-col items-end">
                        <span className="text-[10px] font-mono text-[color:var(--color-text-dim)] flex items-center gap-1">
                          <Clock size={10} /> {fmtDuration(p.opened_age_secs)}
                        </span>
                      </div>
                    </td>
                    <td className="py-3 px-3 text-right">
                      <button
                        onClick={() => { setClosingId(p.id) }}
                        className="px-3 py-1 rounded bg-[color:var(--color-surface-700)] hover:bg-[color:var(--color-bear-600)] text-[color:var(--color-text-primary)] text-[10px] font-bold uppercase tracking-wider transition-all"
                      >
                        Exit
                      </button>
                    </td>
                  </motion.tr>
                )
              })}
            </AnimatePresence>
          </tbody>
        </table>
      </div>
      
      {visible.length === 0 && (
        <div className="py-12 flex flex-col items-center justify-center text-[color:var(--color-text-dim)]">
          <TrendingDown size={24} className="mb-2 opacity-20" />
          <p className="text-xs uppercase tracking-widest font-medium">{emptyLabel}</p>
        </div>
      )}
    </div>
  )
}

export default memo(PositionsTable)
