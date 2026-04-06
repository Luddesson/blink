import { useState, useMemo, memo } from 'react'
import { ChevronDown, ChevronUp } from 'lucide-react'
import type { Position } from '../types'
import { fmt, fmtPnl, pnlClass } from '../lib/format'
import { api } from '../lib/api'

interface Props {
  positions: Position[]
  loading?: boolean
  isLive?: boolean
  onRefresh?: () => void
}

type SortKey = 'market_title' | 'shares' | 'entry_price' | 'current_price' | 'unrealized_pnl'

function PositionsTable({ positions, loading, isLive, onRefresh }: Props) {
  const [sort, setSort] = useState<SortKey>('current_price')
  const [asc, setAsc] = useState(false)
  const [selling, setSelling] = useState<number | null>(null)

  function toggleSort(key: SortKey) {
    if (sort === key) setAsc(!asc)
    else { setSort(key); setAsc(false) }
  }

  const sorted = useMemo(() => [...positions].sort((a, b) => {
    const va = a[sort] as number | string | undefined
    const vb = b[sort] as number | string | undefined
    const sa = va ?? ''
    const sb = vb ?? ''
    if (typeof sa === 'string') return asc ? sa.localeCompare(sb as string) : (sb as string).localeCompare(sa)
    return asc ? (sa as number) - (sb as number) : (sb as number) - (sa as number)
  }), [positions, sort, asc])

  async function handleSell(id: number) {
    if (!confirm('Sell 100% of this position?')) return
    setSelling(id)
    try {
      await api.sellPosition(id, 1.0)
      onRefresh?.()
    } catch (e) {
      alert(`Sell failed: ${e}`)
    } finally {
      setSelling(null)
    }
  }

  function SortIcon({ k }: { k: SortKey }) {
    if (sort !== k) return <span className="opacity-20">↕</span>
    return asc ? <ChevronUp size={12} className="inline" /> : <ChevronDown size={12} className="inline" />
  }

  return (
    <div className="card">
      <div className="flex items-center justify-between mb-3">
        <span className="text-xs font-semibold uppercase tracking-widest text-slate-500">
          Open Positions
        </span>
        <span className="badge badge-neutral">{positions.length}</span>
      </div>

      {loading ? (
        <div className="space-y-2 py-2">
          {[...Array(3)].map((_, i) => (
            <div key={i} className="h-8 bg-surface-700 rounded animate-pulse" />
          ))}
        </div>
      ) : positions.length === 0 ? (
        <p className="text-slate-600 text-xs text-center py-6">No open positions</p>
      ) : (
        <div className="overflow-x-auto">
          <table className="w-full text-xs">
            <thead>
              <tr className="text-slate-500 border-b border-surface-600">
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
                <th className="pb-2 text-right font-normal">Action</th>
              </tr>
            </thead>
            <tbody>
              {sorted.map((p) => (
                <tr key={p.id} className="border-b border-surface-700/50 hover:bg-surface-700/30">
                  <td className="py-2 pr-3 font-mono text-slate-200 max-w-[160px] truncate" title={p.market_title ?? p.token_id}>
                    {p.market_title ?? p.token_id}
                  </td>
                  <td className="py-2 pr-3 font-mono text-slate-300">{p.shares}</td>
                  <td className="py-2 pr-3 font-mono text-slate-300">{fmt(p.entry_price, 4)}</td>
                  <td className="py-2 pr-3 font-mono text-slate-200">{fmt(p.current_price, 4)}</td>
                  <td className={`py-2 pr-3 font-mono font-semibold ${pnlClass(p.unrealized_pnl)}`}>
                    {fmtPnl(p.unrealized_pnl)}
                  </td>
                  <td className="py-2 text-right">
                    <button
                      onClick={() => handleSell(p.id)}
                      disabled={selling === p.id}
                      className="px-2 py-1 rounded text-xs bg-red-900/50 text-red-300 hover:bg-red-800 disabled:opacity-40"
                    >
                      {selling === p.id ? '...' : 'Sell'}
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  )
}

export default memo(PositionsTable)
