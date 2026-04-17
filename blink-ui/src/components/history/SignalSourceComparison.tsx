import { fmt, fmtPnl, fmtDuration } from '../../lib/format'
import type { SourceStats } from '../../hooks/useTradeStats'

interface Props {
  bySignalSource: Record<string, SourceStats>
}

function SourceCard({ name, stats }: { name: string; stats: SourceStats }) {
  const accent = name === 'alpha' ? 'border-t-blue-500/50' : name === 'rn1' ? 'border-t-amber-500/50' : 'border-t-slate-500/50'
  return (
    <div className={`bg-surface-800 ring-1 ring-slate-800/60 rounded-lg p-3 border-t-2 ${accent}`}>
      <div className="flex items-center justify-between mb-2">
        <span className="text-xs font-semibold uppercase tracking-widest text-slate-400">{name}</span>
        <span className="badge badge-neutral text-[10px]">{stats.count} trades</span>
      </div>
      <div className="grid grid-cols-2 gap-3">
        <div>
          <div className="text-[10px] uppercase tracking-wide text-slate-500">Win Rate</div>
          <div className={`text-sm font-mono font-semibold ${stats.winRate >= 50 ? 'text-emerald-400' : 'text-red-400'}`}>
            {fmt(stats.winRate, 1)}%
          </div>
        </div>
        <div>
          <div className="text-[10px] uppercase tracking-wide text-slate-500">Total P&L</div>
          <div className={`text-sm font-mono font-semibold ${stats.totalPnl >= 0 ? 'text-emerald-400' : 'text-red-400'}`}>
            ${fmtPnl(stats.totalPnl)}
          </div>
        </div>
        <div>
          <div className="text-[10px] uppercase tracking-wide text-slate-500">Avg P&L</div>
          <div className={`text-xs font-mono ${stats.avgPnl >= 0 ? 'text-emerald-400' : 'text-red-400'}`}>
            ${fmtPnl(stats.avgPnl)}
          </div>
        </div>
        <div>
          <div className="text-[10px] uppercase tracking-wide text-slate-500">Avg Duration</div>
          <div className="text-xs font-mono text-slate-300">{fmtDuration(stats.avgDuration)}</div>
        </div>
      </div>
    </div>
  )
}

export default function SignalSourceComparison({ bySignalSource }: Props) {
  const sources = Object.entries(bySignalSource)
  if (sources.length === 0) {
    return (
      <div className="card">
        <span className="text-xs font-semibold uppercase tracking-widest text-slate-500 mb-3 block">
          Signal Source Comparison
        </span>
        <p className="text-xs text-slate-500">No data</p>
      </div>
    )
  }

  return (
    <div className="card">
      <span className="text-xs font-semibold uppercase tracking-widest text-slate-500 mb-3 block">
        Signal Source Comparison
      </span>
      <div className="grid grid-cols-2 gap-2">
        {sources.map(([name, stats]) => (
          <SourceCard key={name} name={name} stats={stats} />
        ))}
      </div>
    </div>
  )
}
