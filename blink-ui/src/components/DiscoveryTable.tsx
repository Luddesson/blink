import { useState, useMemo } from 'react'
import type { BullpenDiscoveryResponse, BullpenDiscoveredMarket } from '../types'
import { fmt } from '../lib/format'
import MarketLink from './MarketLink'

interface Props {
  discovery: BullpenDiscoveryResponse | null
}

type SortKey = 'viability_score' | 'seen_count'

const LENS_COLORS: Record<string, string> = {
  momentum: 'bg-blue-800/60 text-blue-300',
  sentiment: 'bg-purple-800/60 text-purple-300',
  volume: 'bg-cyan-800/60 text-cyan-300',
  whale: 'bg-amber-800/60 text-amber-300',
  volatility: 'bg-rose-800/60 text-rose-300',
  flow: 'bg-emerald-800/60 text-emerald-300',
}

const DEFAULT_LENS = 'bg-slate-700/60 text-slate-300'

function lensColor(lens: string): string {
  return LENS_COLORS[lens.toLowerCase()] ?? DEFAULT_LENS
}

function viabilityHsl(score: number): string {
  // 0 = red (0°), 0.5 = yellow (60°), 1.0 = green (120°)
  return `hsl(${Math.round(score * 120)}, 70%, 50%)`
}

export default function DiscoveryTable({ discovery }: Props) {
  const [sortKey, setSortKey] = useState<SortKey>('viability_score')
  const [sortAsc, setSortAsc] = useState(false)

  const markets = useMemo(() => {
    const raw: BullpenDiscoveredMarket[] = discovery?.markets ?? []
    return [...raw].sort((a, b) => {
      const diff = a[sortKey] - b[sortKey]
      return sortAsc ? diff : -diff
    })
  }, [discovery?.markets, sortKey, sortAsc])

  if (!discovery || !discovery.enabled) return null

  function handleSort(key: SortKey) {
    if (sortKey === key) {
      setSortAsc((prev) => !prev)
    } else {
      setSortKey(key)
      setSortAsc(false)
    }
  }

  const sortArrow = (key: SortKey) =>
    sortKey === key ? (sortAsc ? ' ▲' : ' ▼') : ''

  return (
    <div className="bg-slate-900 rounded-lg border border-slate-800 overflow-hidden flex flex-col">
      {/* Scrollable table */}
      <div className="overflow-auto max-h-[480px]">
        <table className="w-full text-[11px]">
          <thead className="sticky top-0 z-10 bg-slate-900 border-b border-slate-800">
            <tr className="text-left text-slate-500 uppercase tracking-wider">
              <th className="px-3 py-2 font-medium">Token ID</th>
              <th className="px-3 py-2 font-medium">Lenses</th>
              <th
                className="px-3 py-2 font-medium cursor-pointer hover:text-slate-300 select-none"
                onClick={() => handleSort('viability_score')}
              >
                Viability{sortArrow('viability_score')}
              </th>
              <th className="px-3 py-2 font-medium">Conv. Boost</th>
              <th className="px-3 py-2 font-medium text-center">Smart $</th>
              <th
                className="px-3 py-2 font-medium cursor-pointer hover:text-slate-300 select-none text-right"
                onClick={() => handleSort('seen_count')}
              >
                Seen{sortArrow('seen_count')}
              </th>
            </tr>
          </thead>
          <tbody>
            {markets.length === 0 ? (
              <tr>
                <td colSpan={6} className="px-3 py-6 text-center text-slate-600">
                  No markets discovered — waiting for next scan
                </td>
              </tr>
            ) : (
              markets.map((m) => {
                const pct = m.viability_score * 100
                return (
                  <tr
                    key={m.token_id}
                    className="border-b border-slate-800/50 hover:bg-slate-800/30"
                  >
                    {/* Token ID */}
                    <td className="px-3 py-1.5">
                      <MarketLink
                        tokenId={m.token_id}
                        label={m.token_id.length > 16 ? `${m.token_id.slice(0, 8)}…${m.token_id.slice(-6)}` : m.token_id}
                        className="font-mono text-slate-300 truncate block max-w-[120px]"
                        titleOverride={m.token_id}
                      />
                    </td>

                    {/* Lenses */}
                    <td className="px-3 py-1.5">
                      <div className="flex flex-wrap gap-1">
                        {m.lenses.map((l) => (
                          <span
                            key={l}
                            className={`px-1.5 py-0.5 rounded text-[9px] font-medium ${lensColor(l)}`}
                          >
                            {l}
                          </span>
                        ))}
                      </div>
                    </td>

                    {/* Viability bar */}
                    <td className="px-3 py-1.5">
                      <div className="flex items-center gap-2">
                        <div className="flex-1 h-1.5 rounded-full bg-slate-800 overflow-hidden max-w-[80px]">
                          <div
                            className="h-full rounded-full"
                            style={{
                              width: `${pct}%`,
                              backgroundColor: viabilityHsl(m.viability_score),
                            }}
                          />
                        </div>
                        <span className="text-slate-300 tabular-nums w-8 text-right">
                          {fmt(pct, 0)}%
                        </span>
                      </div>
                    </td>

                    {/* Conviction Boost */}
                    <td className="px-3 py-1.5 text-slate-400 tabular-nums">
                      {fmt(m.conviction_boost, 2)}
                    </td>

                    {/* Smart Money */}
                    <td className="px-3 py-1.5 text-center">
                      {m.smart_money_interest ? (
                        <span className="text-amber-400">🐋</span>
                      ) : (
                        <span className="text-slate-600">—</span>
                      )}
                    </td>

                    {/* Seen Count */}
                    <td className="px-3 py-1.5 text-right text-slate-400 tabular-nums">
                      {m.seen_count}
                    </td>
                  </tr>
                )
              })
            )}
          </tbody>
        </table>
      </div>
    </div>
  )
}
