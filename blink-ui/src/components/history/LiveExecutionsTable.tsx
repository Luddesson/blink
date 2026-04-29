import { useMemo, useState } from 'react'
import { Search } from 'lucide-react'
import type { LiveExecution } from '../../types'
import { fmt, fmtNeonTime } from '../../lib/format'
import MarketLink from '../MarketLink'

interface Props {
  executions: LiveExecution[]
  realityStatus?: 'matched' | 'mismatch' | 'unverified'
  realityIssues?: string[]
  truthCheckedAtMs?: number | null
  source?: string
}

function shortHash(hash?: string | null) {
  if (!hash) return '--'
  return hash.length > 14 ? `${hash.slice(0, 8)}...${hash.slice(-6)}` : hash
}

function txHref(hash?: string | null) {
  return hash ? `https://polygonscan.com/tx/${hash}` : null
}

function sideClass(side: string) {
  const normalized = side.toUpperCase()
  if (normalized === 'BUY') return 'bg-emerald-900/40 text-emerald-300'
  if (normalized === 'SELL') return 'bg-red-900/40 text-red-300'
  return 'bg-slate-800 text-slate-300'
}

export default function LiveExecutionsTable({
  executions,
  realityStatus,
  realityIssues = [],
  truthCheckedAtMs,
  source,
}: Props) {
  const [search, setSearch] = useState('')
  const isUnverified = realityStatus === 'unverified'
  const checkedAt = truthCheckedAtMs
    ? new Date(truthCheckedAtMs).toLocaleTimeString('sv-SE', {
      hour: '2-digit',
      minute: '2-digit',
      second: '2-digit',
    })
    : null

  const filtered = useMemo(() => {
    const q = search.trim().toLowerCase()
    if (!q) return executions
    return executions.filter((execution) =>
      (execution.market_title ?? execution.token_id).toLowerCase().includes(q)
      || (execution.market_outcome ?? '').toLowerCase().includes(q)
      || execution.side.toLowerCase().includes(q)
      || (execution.transaction_hash ?? '').toLowerCase().includes(q)
    )
  }, [executions, search])

  const totalUsdc = filtered.reduce((sum, execution) => sum + execution.usdc_size, 0)

  return (
    <div className="card">
      <div className="flex items-center justify-between mb-3">
        <div>
          <span className="text-xs font-semibold uppercase tracking-widest text-slate-500">
            Live Wallet Executions
          </span>
          <div className="text-[10px] text-slate-600 mt-0.5">
            {source ?? 'Polymarket activity'}, no simulated or paper fills
          </div>
        </div>
        <div className="flex items-center gap-3">
          <span className="text-[10px] text-slate-500 font-mono">${fmt(totalUsdc, 2)} notional</span>
          <div className="relative">
            <Search size={12} className="absolute left-2 top-1/2 -translate-y-1/2 text-slate-500" />
            <input
              type="text"
              value={search}
              onChange={(event) => setSearch(event.target.value)}
              placeholder="Filter executions..."
              className="bg-surface-700 text-xs text-slate-300 rounded pl-7 pr-2 py-1 w-[180px]
                border border-surface-600 focus:border-cyan-500/50 focus:outline-none"
            />
          </div>
          {realityStatus && (
            <span className={`badge text-[10px] ${
              realityStatus === 'matched'
                ? 'bg-emerald-900/40 text-emerald-300'
                : realityStatus === 'mismatch'
                  ? 'bg-red-900/40 text-red-300'
                  : 'bg-amber-900/40 text-amber-300'
            }`}>
              {realityStatus}
            </span>
          )}
          {checkedAt && <span className="text-[10px] text-slate-500 font-mono">checked {checkedAt}</span>}
          <span className="badge badge-neutral text-[10px]">{filtered.length} executions</span>
        </div>
      </div>

      {filtered.length === 0 ? (
        <p className="text-slate-600 text-xs text-center py-6">
          {isUnverified
            ? `Live wallet activity unverified${realityIssues.length > 0 ? `: ${realityIssues.join(', ')}` : ''}`
            : 'No live wallet executions in this range'}
        </p>
      ) : (
        <div className="overflow-x-auto">
          <table className="w-full text-xs">
            <thead>
              <tr className="text-slate-500 border-b border-surface-600">
                <th className="text-left pb-2 pr-3 font-normal">Market</th>
                <th className="text-left pb-2 pr-3 font-normal">Time</th>
                <th className="text-left pb-2 pr-3 font-normal">Side</th>
                <th className="text-right pb-2 pr-3 font-normal">Size</th>
                <th className="text-right pb-2 pr-3 font-normal">Price</th>
                <th className="text-right pb-2 pr-3 font-normal">USDC</th>
                <th className="text-left pb-2 font-normal">Tx</th>
              </tr>
            </thead>
            <tbody>
              {filtered.map((execution, index) => (
                <tr key={`${execution.transaction_hash ?? execution.token_id}-${index}`} className="border-b border-surface-700/50 hover:bg-surface-700/30">
                  <td className="py-1.5 pr-3 font-mono text-slate-200 max-w-[220px] truncate">
                    <MarketLink
                      tokenId={execution.token_id}
                      label={execution.market_title ?? execution.token_id}
                      titleOverride={execution.market_title ?? execution.token_id}
                    />
                    {execution.market_outcome && (
                      <span className="ml-2 text-[10px] text-slate-500">{execution.market_outcome}</span>
                    )}
                  </td>
                  <td className="py-1.5 pr-3 font-mono text-cyan-400">
                    {execution.traded_at ? fmtNeonTime(execution.traded_at) : '--'}
                  </td>
                  <td className="py-1.5 pr-3">
                    <span className={`text-[9px] font-semibold uppercase px-1.5 py-0.5 rounded-full ${sideClass(execution.side)}`}>
                      {execution.side}
                    </span>
                  </td>
                  <td className="py-1.5 pr-3 font-mono text-slate-300 text-right">
                    {fmt(execution.shares, 4)}
                  </td>
                  <td className="py-1.5 pr-3 font-mono text-slate-300 text-right">
                    ${fmt(execution.price, 4)}
                  </td>
                  <td className="py-1.5 pr-3 font-mono text-slate-300 text-right">
                    ${fmt(execution.usdc_size, 4)}
                  </td>
                  <td className="py-1.5 font-mono text-slate-500">
                    {txHref(execution.transaction_hash) ? (
                      <a
                        href={txHref(execution.transaction_hash) ?? undefined}
                        target="_blank"
                        rel="noreferrer"
                        className="hover:text-cyan-300"
                      >
                        {shortHash(execution.transaction_hash)}
                      </a>
                    ) : shortHash(execution.transaction_hash)}
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
