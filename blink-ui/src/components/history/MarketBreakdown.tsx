import { useState } from 'react'
import SortableTable from '../shared/SortableTable'
import MarketLink from '../MarketLink'
import { fmt, fmtPnl, fmtDuration, pnlClass } from '../../lib/format'
import type { MarketStats } from '../../hooks/useTradeStats'

interface Props {
  byMarket: MarketStats[]
}

export default function MarketBreakdown({ byMarket }: Props) {
  const [showAll, setShowAll] = useState(false)
  const displayed = showAll ? byMarket : byMarket.slice(0, 15)

  const columns = [
    {
      key: 'market',
      label: 'Market',
      render: (r: MarketStats) => (
        <MarketLink tokenId={r.tokenId} label={r.title} maxWidth="200px" />
      ),
      sortKey: (r: MarketStats) => r.title,
      width: '40%',
    },
    {
      key: 'trades',
      label: 'Trades',
      render: (r: MarketStats) => r.count,
      sortKey: (r: MarketStats) => r.count,
      align: 'right' as const,
    },
    {
      key: 'winrate',
      label: 'Win %',
      render: (r: MarketStats) => (
        <span className={r.winRate >= 50 ? 'text-emerald-400' : 'text-red-400'}>
          {fmt(r.winRate, 0)}%
        </span>
      ),
      sortKey: (r: MarketStats) => r.winRate,
      align: 'right' as const,
    },
    {
      key: 'pnl',
      label: 'Total P&L',
      render: (r: MarketStats) => (
        <span className={`font-semibold ${pnlClass(r.totalPnl)}`}>${fmtPnl(r.totalPnl)}</span>
      ),
      sortKey: (r: MarketStats) => r.totalPnl,
      align: 'right' as const,
    },
    {
      key: 'duration',
      label: 'Avg Duration',
      render: (r: MarketStats) => fmtDuration(r.avgDuration),
      sortKey: (r: MarketStats) => r.avgDuration,
      align: 'right' as const,
    },
  ]

  return (
    <div className="card">
      <div className="flex items-center justify-between mb-3">
        <span className="text-xs font-semibold uppercase tracking-widest text-slate-500">
          Market Breakdown
        </span>
        <span className="badge badge-neutral text-[10px]">{byMarket.length} markets</span>
      </div>
      <SortableTable
        columns={columns}
        data={displayed}
        keyFn={r => r.tokenId}
        defaultSort="pnl"
        defaultDir="desc"
        emptyMessage="No trades"
        maxHeight="400px"
      />
      {byMarket.length > 15 && (
        <button
          onClick={() => setShowAll(s => !s)}
          className="mt-2 text-xs text-cyan-400 hover:text-cyan-300"
        >
          {showAll ? 'Show less' : `Show all ${byMarket.length} markets`}
        </button>
      )}
    </div>
  )
}
